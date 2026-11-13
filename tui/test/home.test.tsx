// The home screen: what `paddock` alone opens. Each part is proved here
// against the fake paddock: the header, the choices with keys and the
// mouse, the New agent flow with the folder guard, the apps, the first
// start, 80 columns and less, read-only and plain.
import { afterEach, describe, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { cells } from '../src/clean.js';
import { ArtLayer } from '../src/art-layer.js';
import type { Host } from '../src/host.js';
import { expand, folderRows, lastAgent, NETWORKS, runArgs, tilde, wrapWords, type Job } from '../src/launch.js';
import { halfBlock, rowText } from '../src/art.js';
import { colorOf, spriteFor } from '../src/sprites.js';
import { VERSION } from '../src/version.js';
import { homeChoices, homeLayout, ICON_W, iconRows, moveChoice, stateLine } from '../src/views/home.js';
import { fields, newStart, startKey, verdict, type StartData } from '../src/views/start.js';
import { calls, click, fakeWorld, find, renderApp, until, writeJson, type Rendered } from './helpers.js';

afterEach(() => cleanup());

const UP = '\u001b[A';
const DOWN = '\u001b[B';
const RIGHT = '\u001b[C';
const LEFT = '\u001b[D';
const ESC = '\u001b';

const LIST = {
  harnesses: [
    { name: 'claude', description: 'Anthropic Claude Code', installed: true, app: false },
    { name: 'codex', description: 'OpenAI Codex CLI', installed: true, app: false },
    { name: 'gemini', description: 'Google Gemini CLI', installed: false, app: false },
    { name: 'vscode', description: 'Visual Studio Code', installed: true, app: true },
    { name: 'claude-desktop', description: 'Claude desktop app', installed: false, app: true },
  ],
};

interface Home extends Rendered {
  dir: string;
  /** The commands that the interface handed the terminal to. */
  jobs: Job[];
  /** Folders that exist: the one where paddock started, and two recent ones. */
  here: string;
  api: string;
  site: string;
}

/** The interface as `paddock` alone starts it: on the home screen, with two recent projects. */
async function home(opts: { columns?: number; rows?: number; readonly?: boolean; plain?: boolean; mono?: boolean; art?: ArtLayer; host?: Partial<Host>; edit?: (dir: string) => void } = {}): Promise<Home> {
  let here = '';
  let api = '';
  let site = '';
  const dir = fakeWorld((d) => {
    here = join(d, 'here');
    api = join(d, 'api');
    site = join(d, 'site');
    for (const f of [here, api, site]) mkdirSync(f);
    writeJson(d, 'list.json', LIST);
    writeJson(d, 'recent.json', {
      api_version: 'paddock.recent/v1',
      projects: [
        // Seconds ago (helpers.tsx): api is the newest.
        { project: site, harness: 'claude', last: 200 },
        { project: api, harness: 'codex', last: 100 },
        { project: here, harness: 'codex', last: 300 },
      ],
    });
    opts.edit?.(d);
  });
  const jobs: Job[] = [];
  const r = await renderApp({
    columns: opts.columns,
    rows: opts.rows,
    readonly: opts.readonly,
    plain: opts.plain,
    mono: opts.mono,
    art: opts.art,
    cwd: here,
    host: { home: true, run: (j) => jobs.push(j), ...opts.host },
  });
  return Object.assign(r, { dir, jobs, here, api, site });
}

function fits(r: Rendered, columns: number, rows: number): void {
  const lines = r.lines();
  expect(lines.length).toBeLessThanOrEqual(rows);
  for (const l of lines) expect(cells(l), l).toBeLessThanOrEqual(columns);
}

const explains = (dir: string): string[][] => calls(dir).filter((c) => c.args[0] === 'explain').map((c) => c.args);

describe('the home screen', () => {
  test('80x24: the icon, the version, one line of state, and every choice with its key', async () => {
    const r = await home();
    const f = await r.waitFor('Installed: claude, codex');
    await r.waitFor('3 requests wait for you');
    expect(r.frame()).toContain(`paddock ${VERSION}`);
    // The promise, and one short line under it.
    expect(r.frame()).toContain('Your computer stays yours');
    expect(r.frame()).toContain('Each agent works in its own room. You approve its steps.');
    // The icon stands left of the words, in half blocks.
    expect(r.frame()).toMatch(/[▀▄█]{3}.* paddock \d/);
    for (const [title, key] of [
      ['New agent', 'n'],
      ['Open an app', 'o'],
      ['Requests', 'a'],
      ['Runs', 'r'],
      ['World', 'w'],
      ['Settings', 's'],
      ['Help', '?'],
    ]) {
      expect(f, title).toContain(title);
      expect(f, title).toContain(`key ${key}`);
    }
    // The count of the requests, and what is installed.
    expect(r.frame()).toContain('◆ 3 wait');
    expect(r.frame()).toContain('Installed: vscode');
    expect(r.frame()).toContain('4 live');
    // The first choice is selected, and the footer names the keys.
    expect(r.frame()).toContain('› New agent');
    expect(r.lines().at(-1)).toContain('↵ open');
    // The tabs of the other screens are not here.
    expect(r.frame()).not.toContain('Board');
    fits(r, 80, 24);
    expect(r.lines().length).toBe(24);
  });

  test('the line of state says what runs and what waits', () => {
    const run = (state: string, live = true) => ({ state, live }) as never;
    expect(stateLine([], 0, false)).toBe('Reading paddock…');
    expect(stateLine([], 0, true)).toBe('No agent runs. Press n to start one.');
    expect(stateLine([], 0, true, true)).toBe('No agent runs.');
    expect(stateLine([run('running'), run('running'), run('waiting')], 1, true)).toBe('2 agents working · 1 request waits for you');
    expect(stateLine([run('running'), run('idle'), run('stuck'), run('done', false)], 2, true)).toBe('1 agent working · 1 idle · 1 blocked · 2 requests wait for you');
    expect(stateLine([run('waiting')], 0, true)).toBe('1 agent in a sandbox');
  });

  test('each key opens its screen, and Esc comes back to the home screen', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press('a');
    await r.waitFor('[1] Allow once');
    await r.press('r');
    await r.waitFor('sandbox bwrap');
    await r.press(ESC);
    await r.waitFor('› Requests');
    await r.press('r');
    await r.waitFor('sandbox bwrap');
    expect(r.frame()).not.toContain('key n');
    await r.press('q');
    await r.waitFor('key n');
    await r.press('w');
    await r.waitFor('◇ open');
    await r.press('H');
    await r.waitFor('key n');
    await r.press('?');
    await r.waitFor('Help · every key');
    expect(r.frame()).toContain('the home screen');
    await r.press(ESC);
    await r.waitFor('key n');
    await r.press('s');
    await r.waitFor('Set up paddock');
    await r.press(ESC);
    await r.waitFor('key n');
    await r.press('o');
    await r.waitFor('Open an app · ');
    await r.press(ESC);
    await r.waitFor('key n');
    await r.press('n');
    await r.waitFor('New agent · Tab next');
    await r.press(ESC);
    await r.waitFor('key n');
    expect(r.jobs).toEqual([]);
  });

  test('the arrows move over the tiles, and Enter opens the selected one', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press(RIGHT);
    await r.waitFor('› Open an app');
    await r.press(DOWN);
    // Under the right half of the top row.
    await r.waitFor('› Settings');
    await r.press(LEFT);
    await r.waitFor('› World');
    await r.press(LEFT);
    await r.press(LEFT);
    await r.waitFor('› Requests');
    await r.press(UP);
    await r.waitFor('› New agent');
    // Under the middle of the wide tile.
    await r.press(DOWN);
    await r.waitFor('› Runs');
    await r.press('\r');
    await r.waitFor('sandbox bwrap');
  });

  test('the selection moves by the layout', () => {
    const choices = homeChoices({ readonly: false, agents: [], apps: [], requests: 0, live: 0, ended: 0 });
    const tiles = homeLayout(choices, 80, 22, false);
    expect(tiles).toEqual({ tiles: true, rows: [[0, 1], [2, 3, 4, 5, 6]] });
    expect(moveChoice(tiles, 0, 1, 0)).toBe(1);
    expect(moveChoice(tiles, 0, -1, 0)).toBe(0);
    expect(moveChoice(tiles, 6, 1, 0)).toBe(6);
    expect(moveChoice(tiles, 0, 0, 1)).toBe(3);
    expect(moveChoice(tiles, 1, 0, 1)).toBe(5);
    expect(moveChoice(tiles, 2, 0, -1)).toBe(0);
    expect(moveChoice(tiles, 6, 0, -1)).toBe(1);
    expect(moveChoice(tiles, 6, 0, 1)).toBe(6);
    // A narrow, a low or a plain screen: a list.
    for (const list of [homeLayout(choices, 79, 22, false), homeLayout(choices, 80, 17, false), homeLayout(choices, 120, 40, true)]) {
      expect(list.tiles).toBe(false);
      expect(list.rows).toEqual([[0], [1], [2], [3], [4], [5], [6]]);
    }
    // Read-only has no top row.
    const ro = homeChoices({ readonly: true, agents: [], apps: [], requests: 2, live: 1, ended: 0 });
    expect(ro.map((c) => c.id)).toEqual(['requests', 'runs', 'world', 'help']);
    expect(homeLayout(ro, 80, 22, false).rows).toEqual([[0, 1, 2, 3]]);
  });

  test('the mouse: a click on a tile opens it', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.waitFor('4 live');
    const runs = find(r.frame(), '4 live');
    await r.press(click(runs.x + 1, runs.y));
    await r.waitFor('sandbox bwrap');
    await r.press(ESC);
    await r.waitFor('key n');
    const agent = find(r.frame(), 'Start an agent in a folder');
    await r.press(click(agent.x + 3, agent.y + 1));
    await r.waitFor('New agent · Tab next');
  });

  test('the icon: 19 cells wide, five rows, the colours of the paddock icon or none', () => {
    const plain = iconRows('none');
    expect(plain.length).toBe(5);
    for (const row of plain) expect(Array.from(row).length).toBe(ICON_W);
    // Four pickets with a point each, on two rails.
    expect(plain[1]).toBe('  ▄█▄ ▄█▄ ▄█▄ ▄█▄  ');
    expect(plain[2]).toBe(' ▀███▀███▀███▀███▀ ');
    expect(plain[3]).toBe(' ▀███▀███▀███▀███▀ ');
    expect(plain[0]?.trim()).toBe('');
    expect(plain.join('')).not.toContain('\u001b');
    const colour = iconRows('truecolor');
    expect(colour.length).toBe(5);
    expect(colour.join('')).toContain('38;2;159;184;160');
    expect(colour.join('')).toContain('127;163;132');
    expect(iconRows('256').join('')).toMatch(/38;5;\d+/);
  });

  test.each([
    [79, 24],
    [60, 20],
    [43, 16],
    [100, 30],
    [160, 48],
  ])('%ix%i: every choice shows and fits', async (columns, rows) => {
    const r = await home({ columns, rows });
    await r.waitFor('◆ 3 wait');
    for (const t of ['New agent', 'Open an app', 'Requests', 'Runs', 'World', 'Settings', 'Help']) expect(r.frame(), t).toContain(t);
    if (columns < 80) {
      // A list: one row per choice, with its key in front.
      expect(r.frame()).toMatch(/› n +New agent/);
      expect(r.frame()).toMatch(/ a +Requests +◆ 3 wait/);
      expect(r.frame()).not.toContain('╭');
    }
    fits(r, columns, rows);
    await r.press(DOWN);
    await r.waitFor((f) => (columns < 80 ? /› o +Open an app/.test(f) : f.includes('› Runs')));
    await r.press('n');
    await r.waitFor('Agent');
    await r.waitFor((f) => f.includes('[codex]'));
    fits(r, columns, rows);
  });

  test('--plain: no boxes and no icon, the same choices', async () => {
    const r = await home({ plain: true });
    await r.waitFor('Installed: claude, codex');
    expect(r.frame()).not.toMatch(/[╭│▀▄█]/);
    expect(r.frame()).toContain(`paddock ${VERSION}`);
    expect(r.frame()).toMatch(/› n +New agent/);
    await r.press('n');
    await r.waitFor('[codex]');
    expect(r.frame()).not.toMatch(/[╭│]/);
    fits(r, 80, 24);
  });

  test('NO_COLOR: the icon is the fence in blocks, and the selected tile has its mark', async () => {
    const r = await home({ mono: true });
    await r.waitFor('Installed: claude, codex');
    expect(r.frame()).toContain('▄█▄ ▄█▄ ▄█▄ ▄█▄');
    expect(r.frame()).toContain('▀███▀███▀███▀███▀');
    expect(r.frame()).toContain('› New agent');
    expect(r.frame()).not.toContain('\u001b[3');
    fits(r, 80, 24);
  });

  test('read-only: nothing that starts something', async () => {
    const r = await home({ readonly: true });
    await r.waitFor('◆ 3 wait');
    for (const t of ['New agent', 'Open an app', 'Settings']) expect(r.frame()).not.toContain(t);
    expect(r.frame()).toContain('› Requests');
    for (const k of ['n', 'o', 's']) await r.press(k);
    expect(r.frame()).toContain('› Requests');
    expect(r.frame()).not.toContain('Tab next');
    expect(r.jobs).toEqual([]);
  });

  test('without `paddock` alone the interface starts on the runs, and H opens the home screen', async () => {
    const r = await home({ host: { home: false } });
    await r.waitFor('sandbox bwrap');
    expect(r.frame()).not.toContain('key n');
    await r.press('H');
    await r.waitFor('key n');
    // Esc closes it: the screen before is back.
    await r.press(ESC);
    await r.waitFor('sandbox bwrap');
    expect(r.frame()).not.toContain('key n');
  });

  test('an action of another screen from the palette leaves the home screen', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press(':');
    await r.waitFor('Command palette');
    for (const ch of 'show the board') await r.press(ch);
    await r.press('\r');
    await r.waitFor('◇ open');
    expect(r.frame()).not.toContain('key n');
    // Help from the palette stays over the home screen.
    await r.press('H');
    await r.waitFor('key n');
    await r.press(':');
    await r.waitFor('Command palette');
    for (const ch of 'this help') await r.press(ch);
    await r.press('\r');
    await r.waitFor('Help · every key');
    await r.press(ESC);
    await r.waitFor('key n');
  });

  test('after a command that the home screen started, the home screen shows again, also when the runs are the first screen', async () => {
    const r = await home({ host: { home: false, show: true, notice: { text: 'claude in api ended', role: 'muted' } } });
    await r.waitFor('key n');
    await r.waitFor('claude in api ended');
    // The runs are still the first screen: Esc closes the home screen.
    await r.press(ESC);
    await r.waitFor('sandbox bwrap');
  });

  test('what the last command did shows once', async () => {
    const r = await home({ host: { notice: { text: 'claude in api ended with code 3', role: 'warning' } } });
    await r.waitFor('claude in api ended with code 3');
  });
});

describe('New agent', () => {
  test('the installed agents, the folders, and the defaults: this folder and its last agent', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    const f = await r.waitFor('✓ The agent sees only this folder.');
    // codex ran last in this folder. gemini is not installed, vscode is an app.
    expect(f).toContain('[codex]');
    expect(f).toContain('claude');
    expect(f).not.toContain('gemini');
    expect(f).not.toContain('vscode');
    expect(f).toContain('OpenAI Codex CLI');
    // This folder first, then the recent ones, newest first, each once.
    const rows = r.lines().filter((l) => /[●○] /.test(l));
    expect(rows[0]).toContain(`● ${r.here}`);
    expect(rows[0]).toContain('this folder');
    expect(rows[1]).toContain(`○ ${r.api}`);
    expect(rows[1]).toContain('last: codex');
    expect(rows[2]).toContain(`○ ${r.site}`);
    expect(rows[3]).toContain('○ Other folder: type a path');
    expect(rows.length).toBe(4);
    // The options, in plain words.
    expect(f).toContain('[Default]');
    expect(f).toContain('Your settings decide. Now: internet.');
    // Manual comes first and is picked. Auto is an opt-in, never "recommended".
    expect(f).toContain('[Manual]');
    expect(f).toContain('You approve each step in the agent.');
    expect(f.indexOf('[Manual]')).toBeLessThan(f.indexOf(' Auto '));
    expect(f.toLowerCase()).not.toContain('recommended');
    expect(f).toContain('[ ] Work in a new branch');
    // paddock checked this agent in this folder.
    expect(explains(r.dir).at(-1)).toEqual(['explain', 'codex', r.here, '--json']);
    fits(r, 80, 24);
  });

  test('Start runs `paddock run AGENT FOLDER` in the terminal, with nothing else by default', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    await r.waitFor('✓ The agent sees only this folder.');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs).toEqual([{ args: ['run', 'codex', r.here], title: 'codex in here', pause: 'on-error' }]);
    // The interface itself started nothing: the terminal gets the run.
    expect(calls(r.dir).filter((c) => c.args[0] === 'run')).toEqual([]);
  });

  test('every option reaches the command: agent, folder, network, approvals, branch', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    await r.waitFor('✓ The agent sees only this folder.');
    await r.press(LEFT);
    await r.waitFor('[claude]');
    expect(r.frame()).toContain('Anthropic Claude Code');
    // The folder: down to the list, then to the second recent one.
    await r.press(DOWN);
    await r.press(DOWN);
    await r.press(DOWN);
    await r.waitFor(`● ${r.site}`);
    await r.press('\t');
    for (let i = 0; i < 3; i++) await r.press(RIGHT);
    await r.waitFor('[No network]');
    expect(r.frame()).toContain('No connection at all.');
    await r.press('\t');
    await r.press(' ');
    await r.waitFor('[Auto]');
    expect(r.frame()).toContain('Opt-in: the agent works on its own inside its room.');
    await r.press('\t');
    await r.press(' ');
    await r.waitFor('[x] Work in a new branch');
    expect(r.frame()).toContain('A git worktree. You review the diff.');
    await r.waitFor('✓ The agent sees only this folder.');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs[0]?.args).toEqual(['run', 'claude', r.site, '--network', 'none', '--auto', '--worktree']);
    expect(r.jobs[0]?.title).toBe('claude in site');
    // paddock checked each pair that was picked on the way.
    expect(explains(r.dir).at(-1)).toEqual(['explain', 'claude', r.site, '--json']);
  });

  test('the mouse picks an agent, a folder and the options, and starts', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    await r.waitFor('✓ The agent sees only this folder.');
    const at = (text: string) => find(r.frame(), text);
    let p = at(' claude ');
    await r.press(click(p.x + 2, p.y));
    await r.waitFor('[claude]');
    p = at(`○ ${r.api}`);
    await r.press(click(p.x + 4, p.y));
    await r.waitFor(`● ${r.api}`);
    p = at(' Model only ');
    await r.press(click(p.x + 3, p.y));
    await r.waitFor('[Model only]');
    p = at(' Auto ');
    await r.press(click(p.x + 2, p.y));
    await r.waitFor('[Auto]');
    p = at('[ ] Work in a new branch');
    await r.press(click(p.x + 1, p.y));
    await r.waitFor('[x] Work in a new branch');
    await r.waitFor('✓ The agent sees only this folder.');
    p = at('Start ↵');
    await r.press(click(p.x + 1, p.y));
    await until(() => r.jobs.length > 0);
    expect(r.jobs[0]?.args).toEqual(['run', 'claude', r.api, '--network', 'api', '--auto', '--worktree']);
  });

  test('a typed path: its letters are text, `~` is the home folder, and a folder that is not there does not start', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    await r.waitFor('✓ The agent sees only this folder.');
    for (let i = 0; i < 4; i++) await r.press(DOWN);
    await r.waitFor('● Other folder:');
    // n, q, ?, H and M are commands elsewhere. Here they are letters.
    for (const ch of '../nq?HM') await r.press(ch);
    await r.waitFor('../nq?HM');
    expect(r.frame()).toContain('is not a folder');
    await r.press('\r');
    await r.waitFor('does not exist.');
    expect(r.jobs).toEqual([]);
    for (let i = 0; i < 6; i++) await r.press('\u007f');
    for (const ch of '/api') await r.press(ch);
    await r.waitFor('✓ The agent sees only this folder.');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs[0]?.args).toEqual(['run', 'codex', r.api]);
    expect(expand('~/code', '/x', '/home/demo')).toBe('/home/demo/code');
    expect(expand('~', '/x', '/home/demo')).toBe('/home/demo');
    expect(expand('sub/dir', '/x', '/home/demo')).toBe('/x/sub/dir');
    expect(expand(' /abs/ ', '/x', undefined)).toBe('/abs');
    expect(expand('  ', '/x', '/home/demo')).toBe('');
  });

  test('the folder guard refuses a folder: its reason shows before the start, and Start does nothing', async () => {
    const r = await home({
      edit: (d) =>
        writeJson(d, 'explain-here.json', {
          project: '/home/demo',
          folder_check: { folder: '/home/demo', level: 'refuse', reason: 'it is your home folder: every project and every key is in it', fix: 'Start paddock in the project folder, for example ~/code/myapp' },
        }),
    });
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    const f = await r.waitFor('✖ it is your home folder');
    expect(f).toContain('Start paddock in the project folder');
    expect(f).not.toContain('Yes, start in this wide folder');
    await r.press('\r');
    await r.waitFor((x) => x.split('it is your home folder').length > 2);
    expect(r.jobs).toEqual([]);
    // Another folder is fine, and starts.
    await r.press(DOWN);
    await r.waitFor(`● ${r.api}`);
    await r.waitFor('✓ The agent sees only this folder.');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs[0]?.args).toEqual(['run', 'codex', r.api]);
  });

  test('a wide folder needs a yes: the reason shows, Start waits for the yes, and the yes reaches the command', async () => {
    const r = await home({
      edit: (d) =>
        writeJson(d, 'explain-here.json', {
          project: '/x',
          folder_check: { folder: '/x', level: 'confirm', reason: 'it holds 9 git repositories: an agent would see all of them', fix: 'Start paddock in one of them' },
        }),
    });
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    const f = await r.waitFor('⚠ it holds 9 git repositories');
    expect(f).toContain('Start paddock in one of them');
    expect(f).toContain('[ ] Yes, start in this wide folder');
    await r.press('\r');
    await r.waitFor('This folder is wide. Say yes to it below');
    expect(r.jobs).toEqual([]);
    // Start put the yes in front: one key says it.
    await r.press(' ');
    await r.waitFor('[x] Yes, start in this wide folder');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs[0]?.args).toEqual(['run', 'codex', r.here, '--yes-wide-folder']);
  });

  test('a yes is for one folder: another agent or folder takes it back', () => {
    const d: StartData = {
      agents: [{ name: 'claude' }, { name: 'codex' }],
      folders: [{ path: '/', note: '' }, { path: '/tmp', note: '' }],
      check: { key: 'claude\n/', state: 'done', explain: { approvals: 'manual', folder_check: { level: 'confirm', reason: 'wide' } } },
      cwd: '/',
    };
    const key = (k: Record<string, boolean>) => ({ upArrow: false, downArrow: false, leftArrow: false, rightArrow: false, return: false, escape: false, tab: false, shift: false, ctrl: false, meta: false, backspace: false, delete: false, pageUp: false, pageDown: false, ...k }) as never;
    let s = { ...newStart(), picked: true, wide: true };
    expect(fields(s, d)).toEqual(['agent', 'folder', 'wide', 'network', 'approvals', 'branch']);
    expect(verdict(s, d)).toEqual({ job: { args: ['run', 'claude', '/', '--yes-wide-folder'], title: 'claude in /', pause: 'on-error' } });
    expect(startKey(s, d, '', key({ rightArrow: true })).state).toMatchObject({ agent: 1, wide: false });
    s = { ...s, field: 'folder' };
    const moved = startKey(s, d, '', key({ downArrow: true })).state;
    expect(moved).toMatchObject({ folder: 1, wide: false });
    // No answer of paddock for the new folder yet: Start waits for it.
    expect(verdict(moved, d)).toEqual({ wait: true });
    expect(fields(moved, d)).not.toContain('wide');
    // The mode that paddock would use needs no flag. The other one does.
    s = { ...newStart(), picked: true, wide: true, field: 'approvals' };
    const auto = startKey(s, d, ' ', key({})).state;
    expect(auto.approvals).toBe('auto');
    expect(startKey(auto, d, ' ', key({})).state.approvals).toBe(null);
    // Esc goes back, Enter starts, Tab goes round.
    expect(startKey(s, d, '', key({ escape: true })).act).toBe('back');
    expect(startKey(s, d, '', key({ return: true })).act).toBe('start');
    expect(startKey({ ...s, field: 'branch' }, d, '', key({ tab: true })).state.field).toBe('agent');
    expect(startKey({ ...s, field: 'agent' }, d, '', key({ tab: true, shift: true })).state.field).toBe('branch');
    expect(startKey({ ...s, field: 'agent' }, d, '', key({ upArrow: true })).state.field).toBe('agent');
  });

  test('Start during the check of the folder waits for the answer of paddock', async () => {
    const r = await home({
      edit: (d) => writeJson(d, 'explain-here.json', { folder_check: { level: 'refuse', reason: 'it is a system folder', fix: 'Pick a project' } }),
    });
    await r.waitFor('Installed: claude, codex');
    // n and Enter at once: the answer of paddock is not there yet.
    await r.press('n\r');
    await r.waitFor('✖ it is a system folder');
    await r.waitFor((x) => x.split('it is a system folder').length > 2);
    expect(r.jobs).toEqual([]);
  });

  test('a paddock without the folder guard: nothing to show, and the start works', async () => {
    const r = await home({ edit: (d) => writeJson(d, 'explain.json', { network: 'api', approvals: 'auto' }) });
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    await r.waitFor('Now: model only.');
    expect(r.frame()).toContain('The agent sees only this folder.');
    expect(r.frame()).not.toMatch(/[✓✖⚠] /);
    // The mode of the configuration shows, and needs no flag.
    expect(r.frame()).toContain('[Auto]');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs[0]?.args).toEqual(['run', 'codex', r.here]);
  });

  test('explain fails for another reason: its error shows, and paddock run says the rest in the terminal', async () => {
    const r = await home({ edit: (d) => writeFileSync(join(d, 'explain-exit'), '') });
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    await r.waitFor('⚠ error: no backend can run this sandbox');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs[0]?.args).toEqual(['run', 'codex', r.here]);
  });

  test('each agent has its character', async () => {
    const art = new ArtLayer({ mode: 'truecolor', sink: null, screen: () => ({ columns: 100, rows: 30 }) });
    const r = await home({ columns: 100, rows: 30, art });
    await r.waitFor('Installed: claude, codex');
    await r.press('n');
    await r.waitFor('[codex]');
    // The colours of a character, as the codes that draw them.
    const colours = (agent: string): Set<string> => {
      const out = new Set<string>();
      for (const row of halfBlock(spriteFor(colorOf(agent), 'idle'), 0)) for (const m of rowText(row, 'truecolor').matchAll(/[34]8;2;(\d+;\d+;\d+)/g)) out.add(m[1] as string);
      return out;
    };
    const only = (a: string, b: string): string[] => [...colours(a)].filter((c) => !colours(b).has(c));
    const shows = (f: string, list: string[]): number => list.filter((c) => f.includes(`8;2;${c}m`)).length;
    const blue = only('codex', 'claude');
    const orange = only('claude', 'codex');
    expect(blue.length).toBeGreaterThan(10);
    expect(orange.length).toBeGreaterThan(10);
    // The blue character of codex, then the orange one of claude.
    await r.waitFor((f) => shows(f, blue) > 10);
    expect(shows(r.frame(), orange)).toBe(0);
    expect(r.frame()).toMatch(/ codex +OpenAI|codex {10}/);
    await r.press(LEFT);
    await r.waitFor((f) => shows(f, orange) > 10);
    expect(shows(r.frame(), blue)).toBe(0);
  });

  test('the arguments of the run', () => {
    expect(runArgs({ agent: 'claude', folder: '/p', network: '', approvals: null, branch: false, wide: false })).toEqual(['run', 'claude', '/p']);
    expect(runArgs({ agent: 'claude', folder: '/p', network: 'host', approvals: 'auto', branch: true, wide: true })).toEqual(['run', 'claude', '/p', '--network', 'host', '--auto', '--worktree', '--yes-wide-folder']);
    // Every level is a value that paddock run takes, and has its words.
    expect(NETWORKS.map((n) => n.id)).toEqual(['', 'api', 'egress', 'none', 'host']);
    for (const n of NETWORKS) expect(n.words.length).toBeLessThanOrEqual(38);
    expect(NETWORKS.filter((n) => n.wide).map((n) => n.id)).toEqual(['host']);
  });

  test('folders: this folder first, each once, newest first, and the last agent of a folder', () => {
    const recent = [
      { project: '/b', harness: 'codex', last: 1 },
      { project: '/a', harness: 'claude', last: 3 },
      { project: '/here', harness: 'gemini', last: 9 },
      { project: '/a', harness: 'codex', last: 2 },
      { project: '', harness: 'x' },
    ];
    expect(folderRows('/here', recent)).toEqual([
      { path: '/here', note: 'this folder' },
      { path: '/a', note: 'last: claude' },
      { path: '/b', note: 'last: codex' },
    ]);
    expect(lastAgent('/a', recent, ['codex', 'claude'])).toBe(1);
    // gemini is not installed: the first agent.
    expect(lastAgent('/here', recent, ['codex', 'claude'])).toBe(0);
    expect(lastAgent('/none', recent, ['codex', 'claude'])).toBe(0);
    expect(tilde('/home/demo/code/app', '/home/demo')).toBe('~/code/app');
    expect(tilde('/home/demo', '/home/demo')).toBe('~');
    expect(tilde('/srv/demo/x', '/home/demo')).toBe('/srv/demo/x');
    expect(tilde('/x', undefined)).toBe('/x');
    expect(wrapWords('one two three four five', 9, 2)).toEqual(['one two', 'three four five']);
    expect(wrapWords('one two', 20, 2)).toEqual(['one two']);
  });
});

describe('the first start', () => {
  const NONE = { harnesses: LIST.harnesses.map((h) => ({ ...h, installed: false })).concat([{ name: 'amp', description: 'Amp', installed: false, app: false }]) };

  test('no agent is installed: which ones paddock knows, how to install them, and R looks again', async () => {
    const r = await home({ edit: (d) => writeJson(d, 'list.json', NONE) });
    await r.waitFor('None is installed yet. See how.');
    await r.press('n');
    const f = await r.waitFor('no agent is installed yet');
    expect(f).toContain('It found none.');
    expect(f).toMatch(/claude +Anthropic Claude Code/);
    expect(f).toContain('npm install -g @anthropic-ai/claude-code');
    expect(f).toContain('npm install -g @openai/codex');
    expect(f).toContain('npm install -g @google/gemini-cli');
    // An agent without a known command is named. An app is not an agent.
    expect(f).toContain('Also: amp.');
    expect(f).not.toContain('vscode');
    // Enter starts nothing here.
    await r.press('\r');
    expect(r.jobs).toEqual([]);
    fits(r, 80, 24);
    // The user installs one and presses R.
    writeJson(r.dir, 'list.json', LIST);
    await r.press('R');
    await r.waitFor('[codex]');
    await r.waitFor('✓ The agent sees only this folder.');
  });
});

describe('Open an app', () => {
  test('the apps of paddock list, installed first: Enter opens one in the terminal', async () => {
    const r = await home();
    await r.waitFor('Installed: vscode');
    await r.press('o');
    const f = await r.waitFor('Visual Studio Code');
    const rows = r.lines().filter((l) => /vscode|claude-desktop/.test(l));
    expect(rows[0]).toMatch(/› vscode +Visual Studio Code/);
    expect(rows[1]).toMatch(/claude-desktop +Claude desktop app +· not installed/);
    // Agents are not apps.
    expect(f).not.toContain('OpenAI Codex CLI');
    await r.press(DOWN);
    await r.waitFor('› claude-desktop');
    await r.press('\r');
    await r.waitFor('claude-desktop is not installed on this computer');
    expect(r.jobs).toEqual([]);
    await r.press(UP);
    await r.waitFor('› vscode');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs).toEqual([{ args: ['app', 'run', 'vscode'], title: 'vscode', pause: 'on-error' }]);
    fits(r, 80, 24);
  });

  test('no app: the page says so', async () => {
    const r = await home({ edit: (d) => writeJson(d, 'list.json', { harnesses: LIST.harnesses.filter((h) => !h.app) }) });
    await r.waitFor('No app is installed.');
    await r.press('o');
    await r.waitFor('paddock knows no desktop app on this computer.');
  });
});

describe('Settings', () => {
  test('each row runs one paddock command in the terminal, and its output stays until Enter', async () => {
    const r = await home();
    await r.waitFor('Installed: claude, codex');
    await r.press('s');
    const f = await r.waitFor('Check this computer');
    expect(f).toContain('~/.config/paddock/config.toml');
    await r.press(DOWN);
    await r.waitFor('› Check this computer');
    await r.press('\r');
    await until(() => r.jobs.length > 0);
    expect(r.jobs).toEqual([{ args: ['doctor'], title: 'paddock doctor', pause: 'always' }]);
    fits(r, 80, 24);
  });
});
