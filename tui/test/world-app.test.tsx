// The world in the interface: the Board tab is the ranch, Enter zooms in
// and Esc out, P turns the Plumbing view on, I opens the Inspector, and
// each action runs the paddock command that the Inspector names.
import React, { Profiler } from 'react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { cleanup, render } from 'ink-testing-library';
import { App } from '../src/app.js';
import { ArtContext, ArtLayer } from '../src/art-layer.js';
import { cells } from '../src/clean.js';
import { paddock, type PaddockApi } from '../src/paddock.js';
import { Painter } from '../src/screen.js';
import { makeTheme, ThemeContext } from '../src/theme.js';
import { reducedMotion } from '../src/main.js';
import { actions, calls, click, fakeWorld, renderApp, rounds, sleep, until, writeJson, type Rendered } from './helpers.js';
import { demo } from './world-demo.js';
import { AMBER } from '../src/world/pixels.js';

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

function strip(s: string): string {
  return s.replace(/\u001b\[[\d;]*m/g, '');
}

/** Wait until a text is on the screen. Ink styles each word by itself, so the codes are taken out first. */
async function see(r: Rendered, text: string): Promise<string> {
  return strip(await r.waitFor((f) => strip(f).includes(text)));
}

function art(columns: number, rows: number, mode: 'truecolor' | '256' = 'truecolor'): ArtLayer {
  return new ArtLayer({ mode, sink: null, screen: () => ({ columns, rows }) });
}

/** The top line of the world: the breadcrumb and what waits. */
function crumb(r: Rendered): string {
  return strip(r.lines()[1] ?? '');
}

/** Where a text stands on the screen, in cells. */
function at(r: Rendered, text: string): { x: number; y: number } {
  const lines = r.lines().map(strip);
  for (let y = 0; y < lines.length; y++) {
    const i = (lines[y] as string).indexOf(text);
    if (i >= 0) return { x: cells((lines[y] as string).slice(0, i)), y };
  }
  throw new Error(`"${text}" is not on the screen:\n${lines.join('\n')}`);
}

function fits(r: Rendered, columns: number, rows: number): void {
  const lines = r.lines();
  expect(lines.length).toBeLessThanOrEqual(rows);
  for (const l of lines) expect(cells(strip(l)), strip(l)).toBeLessThanOrEqual(columns);
}

const DEVICES = {
  devices: [
    { name: 'screenshot', enabled: true, run: { run: 'host' }, scope: 'run', mode: 'ask' },
    { name: 'lint', enabled: true, run: { run: 'sandbox' }, scope: 'run', mode: 'allow' },
    { name: 'train', enabled: true, run: { run: 'remote', target: 'ssh://gpu-1' }, scope: 'run', mode: 'allow' },
  ],
};

/** The interface on the Board tab. With art that is the ranch; `art: undefined` gives the list (no art). */
async function ranch(o: Parameters<typeof renderApp>[0] & { columns: number; rows: number }): Promise<Rendered> {
  const r = await renderApp({ art: art(o.columns, o.rows), ...o });
  await r.waitFor((f) => strip(f).includes('Runs · 4 live'));
  await r.press('b');
  if ('art' in o && o.art === undefined) await r.waitFor((f) => strip(f).includes('Board · projects'));
  else await r.waitFor((f) => strip(f).split('\n')[1]?.startsWith('Ranch ') === true && strip(f).includes('ask you'));
  return r;
}

describe('the ranch is the board', () => {
  test.each([
    [80, 24],
    [100, 30],
    [200, 60],
  ])('%i x %i: b shows the ranch, in art, and it fits', async (columns, rows) => {
    fakeWorld();
    const r = await ranch({ columns, rows });
    const f = r.frame();
    const plain = strip(f);
    expect(crumb(r)).toMatch(/^Ranch +◆ 3 ask you$/);
    // The houses of the projects with their signs, each run under its room, the build bench.
    // A wide screen has room for the whole name of a run.
    expect(plain).toContain(' alpha ');
    expect(plain).toMatch(/codex(-0b0b0b)? ◆/);
    expect(plain).toMatch(/claude(-71c3ac)? ◆ ✉2/);
    // The tickets are not in the house: its sign has them (the next test).
    for (const w of ['waiting T-101', 'working T-102', 'open T-103']) expect(plain, w).not.toContain(w);
    if (columns >= 100) for (const w of ['beta', 'Build bench', '1 claude-alpha-71c3ac']) expect(plain, w).toContain(w);
    // The art, in 24-bit colour: half blocks, or whole cells where a pixel of the art is twice as large (200 columns).
    if (columns < 160) expect(f).toContain('▀');
    else expect(f).toMatch(/\u001b\[[\d;]*48;2;162;232;149m/);
    expect(f).toMatch(/\u001b\[[\d;]*38;2;\d+;\d+;\d+/);
    // The first thing selected is a room that asks you; the last line says what it is.
    expect(plain).toMatch(/codex-0b0b0b · ◆ asks you \d+ s · "git push"/);
    expect(plain).toContain('←→↑↓ move');
    expect(plain).toContain('P plumbing');
    fits(r, columns, rows);
  });

  test('the sign of a house has its tickets: the line at the bottom names them, and the Inspector lists them', async () => {
    fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    // Down from the room: the sign under the house.
    await r.press('\u001b[B');
    await r.waitFor((f) => strip(f).includes('Tickets of alpha: T-101 waiting, T-102 working, T-103 open'));
    await r.press('I');
    await r.waitFor((f) => strip(f).includes('paddock: board tickets'));
    for (const w of ['Signpost', '$ paddock board --json', '"id": "T-101"']) expect(strip(r.frame()), w).toContain(w);
  });

  test('256 colours: the ranch in the 256-colour codes', async () => {
    fakeWorld();
    const r = await ranch({ columns: 80, rows: 24, art: art(80, 24, '256') });
    expect(r.frame()).toMatch(/\u001b\[[\d;]*38;5;\d+/);
    expect(r.frame()).not.toMatch(/[34]8;2;/);
  });

  test('Enter zooms in, Esc goes one stop out, and at the ranch back to the runs', async () => {
    fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('\r');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha '));
    // A project without a loft: a small sign under the house says so.
    expect(strip(r.frame())).toContain(' alpha   No notes yet');
    expect(strip(r.frame())).toContain('› codex-0b0b0b ◆ asks you');
    await r.press('\r');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b '));
    const room = strip(r.frame());
    expect(room).toMatch(/Internet +model only/);
    expect(room).toMatch(/Asks you +git push/);
    fits(r, 100, 30);
    await r.press('\u001b');
    await r.waitFor(() => /^Ranch › alpha +◆/.test(crumb(r)));
    // The room that was open is the selected one.
    expect(strip(r.frame())).toContain('› codex-0b0b0b ◆ asks you');
    await r.press('\u001b');
    await r.waitFor(() => /^Ranch +◆/.test(crumb(r)));
    await r.press('\u001b');
    await r.waitFor((f) => strip(f).includes('Runs · 4 live'));
  });

  test('the arrows move the selection; 1, 2 and 3 jump to the ranch, the building and the room', async () => {
    fakeWorld();
    const r = await ranch({ columns: 120, rows: 34 });
    await r.press('\u001b[C');
    await r.waitFor((f) => strip(f).includes('claude-71c3ac · ◆ asks you'));
    await r.press('\u001b[D');
    await r.waitFor((f) => strip(f).includes('codex-0b0b0b · ◆ asks you'));
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    await r.press('2');
    await r.waitFor(() => /^Ranch › alpha +◆/.test(crumb(r)));
    await r.press('1');
    await r.waitFor(() => /^Ranch +◆/.test(crumb(r)));
    expect(strip(r.frame())).toContain('codex-0b0b0b · ◆ asks you');
  });

  test('l goes up to the loft of the project, and Esc comes down', async () => {
    fakeWorld((dir) => {
      const d = demo();
      writeJson(dir, 'ps.json', d.ps);
      writeJson(dir, 'board.json', d.board);
      writeJson(dir, 'approvals.json', { pending: d.pending });
    });
    const r = await renderApp({ columns: 100, rows: 30, art: art(100, 30) });
    await r.waitFor((f) => strip(f).includes('Runs · 5 live'));
    await r.press('b');
    await r.waitFor(() => crumb(r).startsWith('Ranch '));
    expect(strip(r.frame())).toContain('loft ▲');
    await r.press('l');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › Loft'));
    expect(strip(r.frame())).toContain('Folder  /data/loft/abc/tree/.paddock');
    await r.press('\u001b');
    await r.waitFor(() => /^Ranch › alpha +◆/.test(crumb(r)));
  });

  test('a project without a loft has no way up', async () => {
    fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    expect(strip(r.frame())).not.toContain('loft ▲');
    await r.press('l');
    await see(r, 'This project has no loft');
    expect(crumb(r)).toMatch(/^Ranch +◆/);
  });

  test('L gives the board as a list, and the ranch again', async () => {
    fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('L');
    const f = await see(r, 'Board · projects');
    expect(f).toContain('The board as a list');
    expect(f).toContain('compile slots: 1 claude-alpha-71c3ac');
    await r.press('L');
    await r.waitFor(() => crumb(r).startsWith('Ranch '));
  });
});

describe('the Plumbing view and the Inspector', () => {
  test('Plumbing is off at first; P shows every tool, and the tool on your machine shows in both views', async () => {
    fakeWorld((dir) => writeJson(dir, 'device.json', DEVICES));
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    await r.waitFor((f) => strip(f).includes('screenshot'));
    let f = strip(r.frame());
    expect(crumb(r)).not.toContain('Plumbing');
    expect(f).toMatch(/Tools on your machine/);
    expect(f).toMatch(/screenshot +your machine · ask/);
    expect(f).not.toContain('lint');
    expect(f).not.toContain('gpu-1');
    // The amber of the cable to your machine.
    expect(r.frame()).toContain(`2;${AMBER[0]};${AMBER[1]};${AMBER[2]}`);
    await r.press('P');
    await r.waitFor(() => crumb(r).includes('Plumbing'));
    f = strip(r.frame());
    expect(f).toMatch(/lint +in the room/);
    expect(f).toMatch(/train +gpu-1/);
    expect(f).toMatch(/Borrowed folder +paddock has no list yet/);
    expect(f).toContain('Plumbing: glass walls, tools and cables');
    await r.press('p');
    await r.waitFor(() => !crumb(r).includes('Plumbing'));
    expect(strip(r.frame())).not.toContain('lint');
    expect(strip(r.frame())).toMatch(/screenshot +your machine · ask/);
  });

  test('I shows the paddock word, the command and the JSON of the selected thing; c copies the command', async () => {
    fakeWorld();
    const copied: string[] = [];
    const r = await ranch({ columns: 100, rows: 30, onCopy: (t) => copied.push(t) });
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    await r.press('I');
    let f = await see(r, 'Inspector · c copies');
    for (const w of ['Asks you', 'paddock: approval request', '$ paddock approvals --json', 'paddock approvals answer r-1-net', 'once|session|deny', '↵  open the card: once, this session, deny', '"id": "r-1-net"']) expect(f, w).toContain(w);
    // The escape sequences in the request never reach the screen.
    expect(r.frame()).not.toContain('\u001b]0;');
    await r.press('c');
    await see(r, 'Copied: paddock approvals --json');
    expect(copied).toEqual(['paddock approvals --json']);
    // The Inspector follows the selection.
    await r.press('\u001b[A');
    f = await see(r, 'paddock: network mode api');
    expect(f).toContain('One pipe: only the service of its model.');
    expect(f).toContain('Not yet: ps does not list the allowed hosts');
    // Esc closes the Inspector first; the room stays.
    await r.press('\u001b');
    await r.waitFor((x) => !strip(x).includes('Inspector ·'));
    expect(crumb(r)).toContain('codex-0b0b0b');
    // On the ranch.
    await r.press('1');
    await r.waitFor(() => /^Ranch +◆/.test(crumb(r)));
    await r.press('i');
    f = await see(r, 'Inspector · c copies');
    expect(f).toContain('paddock: run codex-0b0b0b in a sandbox');
    expect(f).toContain('$ paddock ps --json');
    fits(r, 100, 30);
  });

  test('without a way to copy, c shows the command', async () => {
    fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('c');
    await see(r, 'paddock ps --json');
    expect(strip(r.frame())).not.toContain('Copied');
  });
});

describe('the actions of the world', () => {
  test('Enter on the door opens the card of the request; its key answers with the command of the Inspector', async () => {
    const dir = fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    await r.press('\r');
    const f = await see(r, '[1] Allow once');
    for (const w of ['◆ Asks you', 'codex wants to run a command:', '│ exec: git push origin main', '[2] Allow for this run', '[3] Deny', 'Esc closes it: no answer', 'Esc close the card']) expect(f, w).toContain(w);
    // The room stays beside the card, whole.
    expect(f).toContain('Ranch › alpha › codex-0b0b0b');
    fits(r, 100, 30);
    await r.press('1');
    await until(() => actions(dir).length > 0);
    expect(actions(dir).map((c) => c.args)).toEqual([['approvals', 'answer', 'r-1-net', 'once']]);
    const after = await see(r, 'Answered codex-alpha-0b0b0b11: Allow once');
    // The card is gone with its request. The request of another run does not take its place in this room.
    expect(after).not.toContain('[1] Allow once');
    expect(after).not.toContain('Which ledger format?');
    expect(after).toMatch(/Asks you +nothing now/);
  });

  test('80 x 24: the card lies beside the door, and the whole room still shows', async () => {
    fakeWorld();
    const r = await ranch({ columns: 80, rows: 24, mono: true, art: undefined });
    await r.press('L');
    await r.waitFor(() => crumb(r).startsWith('Ranch '));
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    await r.press('\r');
    const f = await see(r, '[1] Allow once');
    for (const w of ['▛▀▀', '▙▄▄', '▐!', '◕ !', '[4] Deny for this run']) expect(f, w).toContain(w);
    fits(r, 80, 24);
  });

  test('the arrows choose an answer and Enter gives it; a click on an answer gives it too', async () => {
    const dir = fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    await r.press('\r');
    await see(r, '[1] Allow once');
    // The focus starts on Deny: the safe answer. Up goes to "Allow for this run".
    await r.press('\u001b[A');
    await see(r, 'remember: this run only');
    await r.press('\r');
    await until(() => actions(dir).length === 1);
    expect(actions(dir)[0]?.args).toEqual(['approvals', 'answer', 'r-1-net', 'session']);
  });

  test('a click on an answer of the card gives it', async () => {
    const dir = fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    await r.press('\r');
    await see(r, '[1] Allow once');
    const deny = at(r, '[3] Deny');
    await r.press(click(deny.x + 2, deny.y));
    await until(() => actions(dir).length === 1);
    expect(actions(dir)[0]?.args).toEqual(['approvals', 'answer', 'r-1-net', 'deny']);
  });

  test('Esc closes the card and does not answer it', async () => {
    const dir = fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    await r.press('\r');
    await see(r, '[1] Allow once');
    await r.press('\u001b');
    await r.waitFor((f) => !strip(f).includes('[1] Allow once'));
    await rounds(dir, 2);
    expect(actions(dir)).toEqual([]);
    expect(crumb(r)).toContain('codex-0b0b0b');
  });

  test('z pauses the selected room with paddock freeze, and resumes a paused one with paddock thaw', async () => {
    const dir = fakeWorld((d) => {
      const ps = demo().ps;
      // Times in seconds ago. The frozen run is the oldest: the first room of its house.
      writeJson(d, 'ps.json', { ...ps, sandboxes: ps.sandboxes.map((s, i) => (s.frozen_since ? { ...s, started: 9000, frozen_since: 100 } : { ...s, started: 2000 + i })) });
      writeJson(d, 'board.json', demo().board);
      writeJson(d, 'approvals.json', { pending: [] });
    });
    const r = await renderApp({ columns: 120, rows: 34, art: art(120, 34) });
    await r.waitFor((f) => strip(f).includes('Runs · 5 live'));
    await r.press('b');
    // The first room is the frozen one.
    await r.waitFor((f) => strip(f).includes('grok-0f0f0f · ❄ paused'));
    await r.press('z');
    await until(() => actions(dir).length === 1);
    expect(actions(dir)[0]?.args).toEqual(['thaw', 'grok-alpha-0f0f0f22']);
    await r.press('\u001b[C');
    await r.waitFor((f) => strip(f).includes('codex-0b0b0b ·'));
    await r.press('z');
    await until(() => actions(dir).length === 2);
    expect(actions(dir)[1]?.args).toEqual(['freeze', 'codex-alpha-0b0b0b11']);
  });

  test('Enter on the mailbox opens the messages of the run, and Esc comes back to the room', async () => {
    fakeWorld();
    const r = await ranch({ columns: 100, rows: 30 });
    await r.press('\u001b[C');
    await r.waitFor((f) => strip(f).includes('claude-71c3ac · ◆ asks you'));
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › claude-71c3ac'));
    expect(strip(r.frame())).toMatch(/Messages +2 unread/);
    await r.press('\u001b[B');
    await r.waitFor((f) => strip(f).includes('Messages: 2 unread'));
    await r.press('\r');
    await see(r, 'Mail · claude-alpha-71c3ac9d');
    await r.press('\u001b');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › claude-71c3ac'));
  });

  test('--readonly: the world shows, and nothing in it changes anything', async () => {
    const dir = fakeWorld();
    const r = await ranch({ columns: 100, rows: 30, readonly: true });
    expect(strip(r.frame())).not.toContain('z pause');
    await r.press('z');
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    // The door opens the Inspector, not the card.
    await r.press('\r');
    const f = await see(r, 'Inspector · c copies');
    expect(f).not.toContain('[1] Allow once');
    expect(f).not.toContain('open the card');
    await rounds(dir, 2);
    expect(actions(dir)).toEqual([]);
  });

  test('a click selects a thing, a double click opens it', async () => {
    fakeWorld();
    const r = await ranch({ columns: 120, rows: 34 });
    const claude = at(r, 'claude ◆');
    await r.press(click(claude.x + 1, claude.y - 2));
    await r.waitFor((f) => strip(f).includes('claude-71c3ac · ◆ asks you'));
    await r.press(click(claude.x + 1, claude.y - 2));
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha '));
    expect(strip(r.frame())).toContain('› claude-71c3ac ◆ asks you');
  });
});

describe('the reads of the world', () => {
  test('the JSON of the world is read only while the world shows', async () => {
    const dir = fakeWorld();
    const r = await renderApp({ columns: 100, rows: 30, art: art(100, 30) });
    await r.waitFor((f) => strip(f).includes('Runs · 4 live'));
    await rounds(dir, 2);
    const world = ['device ls --json', 'kv overview --json', 'files --json', 'forage --json', 'horse list --json'];
    const read = (): string[] => calls(dir).map((c) => c.args.join(' ')).filter((a) => world.includes(a));
    expect(read()).toEqual([]);
    await r.press('b');
    await until(() => world.every((w) => read().includes(w)));
    // Back at the runs: no further read.
    await r.press('r');
    await r.waitFor((f) => strip(f).includes('Runs · 4 live'));
    await sleep(100);
    const n = read().length;
    await rounds(dir, 6);
    expect(read().length).toBe(n);
  });

  test('a paddock without a command of the world: that part stays out, and the command is not run again', async () => {
    const dir = fakeWorld((d) => {
      for (const c of ['device', 'kv', 'files', 'forage', 'horse']) writeJson(d, `unsupported-${c}`, '');
    });
    const r = await ranch({ columns: 100, rows: 30 });
    await until(() => calls(dir).some((c) => c.args[0] === 'horse'));
    await rounds(dir, 12);
    for (const c of ['device', 'kv', 'files', 'forage', 'horse']) expect(calls(dir).filter((x) => x.args[0] === c), c).toHaveLength(1);
    const f = strip(r.frame());
    expect(f).toContain('alpha');
    expect(f).not.toContain('Ranch gate');
    // No error line: a command that this paddock lacks is not an error of the world.
    expect(f).not.toContain('newer paddock');
  });
});

describe('without art', () => {
  test('NO_COLOR: the board is the list, and L gives the ranch as clean text', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 80, rows: 24, mono: true });
    await see(r, 'sandbox bwrap');
    await r.press('b');
    await see(r, 'Board · projects');
    await r.press('L');
    await r.waitFor(() => crumb(r).startsWith('Ranch '));
    const f = r.frame();
    const plain = strip(f);
    for (const w of ['▄▄ alpha ▄', '▛▀▀▀▀▀▀▀▀▀▀▜', '[model]', '◕!', 'asks you', '◇ open T-103', 'codex ◆']) expect(plain, w).toContain(w);
    // No colour code at all: only bold, dim, underline and inverse.
    expect(f).not.toMatch(/\u001b\[[\d;]*(3\d|4\d|9\d|10\d)[;m]/);
    expect(f).not.toContain('▀▄'.repeat(0) + '\u001b[38');
    fits(r, 80, 24);
    // The room, with its words.
    await r.press('3');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha › codex-0b0b0b'));
    const room = strip(r.frame());
    expect(room).toMatch(/╞═▶ Internet +model only/);
    expect(room).toContain('▐!');
    expect(r.frame()).not.toMatch(/\u001b\[[\d;]*(3\d|4\d|9\d|10\d)[;m]/);
    fits(r, 80, 24);
  });

  test('--plain: the ranch without any escape sequence in its picture', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 80, rows: 24, plain: true, mouse: false });
    await see(r, 'sandbox bwrap');
    await r.press('b');
    await r.press('L');
    await r.waitFor(() => crumb(r).startsWith('Ranch '));
    const scene = r.lines().slice(1, 22).join('\n');
    expect(scene).not.toContain('\u001b');
    expect(scene).toContain('▄▄ alpha ▄');
  });

  test('a screen that is too small for the world says so', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 50, rows: 20 });
    await see(r, 'Runs · 4 live');
    await r.press('b');
    await r.press('L');
    await see(r, 'The world needs 60 columns and 11 rows');
  });

  test('PADDOCK_TUI_MOTION turns the motion off', () => {
    expect(['reduce', 'off', '0', 'REDUCE'].map((v) => reducedMotion({ PADDOCK_TUI_MOTION: v }))).toEqual([true, true, true, true]);
    expect([{}, { PADDOCK_TUI_MOTION: 'on' }, { PADDOCK_TUI_MOTION: '' }].map((e) => reducedMotion(e))).toEqual([false, false, false]);
  });
});

// The cost of the world with 20 rooms on a 200 x 60 screen, with a fake
// clock, as in perf.test.tsx.
test('the world moves at 8 frames a second without a React render, and rests in the background', async () => {
  const START = Math.floor(Date.now() / 1000) * 1000;
  const realTimeout = setTimeout;
  const breathe = () => new Promise<void>((r) => realTimeout(r, 0));
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'], now: START });
  const now = START / 1000;
  const harnesses = ['claude', 'codex', 'gemini', 'copilot', 'grok', 'opencode', 'aider'];
  const ps = {
    sandboxes: Array.from({ length: 20 }, (_, i) => ({
      id: `${harnesses[i % 7]}-proj${i % 4}-${String(i).padStart(2, '0')}aabbcc`,
      short: `${String(i).padStart(2, '0')}aabb`,
      harness: harnesses[i % 7],
      project: `/src/proj${i % 4}`,
      backend: 'bwrap',
      network: 'egress',
      started: now - 3600 * (1 + (i % 5)) - 100,
    })),
    ended: [],
  };
  const api: PaddockApi = {
    ...paddock,
    ps: async () => JSON.parse(JSON.stringify(ps)) as never,
    approvals: async () => ({ pending: [] }),
    board: async () => ({ projects: [] }),
    mailBoxes: async () => [],
    hasSplit: async () => false,
    harnesses: async () => [],
    devices: async () => ({ devices: [] }),
    kvOverview: async () => ({ pastures: [] }),
    files: async () => ({ sandboxes: [] }),
    forage: async () => ({ windows: [] }),
    horses: async () => [],
  };
  const size = { columns: 200, rows: 60 };
  const commits: number[] = [];
  const stepMs: number[] = [];
  let written = 0;
  const painter: Painter = new Painter({ write: (s) => ((written += s.length), true), size: () => size, overlays: () => layer.overlays(), minFrameMs: 0 });
  const layer: ArtLayer = new ArtLayer({ mode: 'truecolor', sink: null, screen: () => size, painter, onStep: (ms) => stepMs.push(ms) });
  const inst = render(
    <Profiler id="app" onRender={(_id, _phase, actual) => commits.push(actual)}>
      <ThemeContext.Provider value={makeTheme('dark')}>
        <ArtContext.Provider value={layer}>
          <App api={api} initialMouse intervalMs={2000} size={size} />
        </ArtContext.Provider>
      </ThemeContext.Provider>
    </Profiler>,
  );
  Object.defineProperty(inst.stdout, 'columns', { value: 200, configurable: true });
  Object.defineProperty(inst.stdout, 'rows', { value: 60, configurable: true });
  const inkWrite = inst.stdout.write;
  let inkFrames = 0;
  inst.stdout.write = (f: string) => {
    inkFrames++;
    inkWrite(f);
    queueMicrotask(() => painter.frame(f));
  };
  const pass = async (ms: number, step = 125): Promise<void> => {
    for (let t = 0; t < ms; t += step) {
      await vi.advanceTimersByTimeAsync(step);
      await breathe();
    }
  };
  for (let i = 0; i < 40 && !(inst.lastFrame() ?? '').includes('1/20'); i++) await pass(250);
  inst.stdin.write('b');
  for (let i = 0; i < 40 && !painter.text().some((l) => l.startsWith('Ranch')); i++) await pass(250);
  expect(painter.text()[1]).toMatch(/^Ranch/);
  // The painter draws the world: Ink only keeps its place free.
  expect(layer.overlays()).toHaveLength(1);
  expect(painter.text().join('\n')).toContain('proj0');
  // The first look pulses and settles.
  await pass(3000);

  // With the focus: 8 steps a second, each a patch of the changed cells, and no React render.
  const anim = { commits: commits.length, ink: inkFrames, steps: layer.steps, cells: painter.stats.cells };
  stepMs.length = 0;
  await pass(10_000);
  const steps = layer.steps - anim.steps;
  expect(steps).toBeGreaterThanOrEqual(79);
  expect(steps).toBeLessThanOrEqual(81);
  expect(commits.length - anim.commits).toBe(0);
  expect(inkFrames - anim.ink).toBe(0);
  // Far fewer cells than the screen has (12 000): only the characters move.
  expect((painter.stats.cells - anim.cells) / steps).toBeLessThan(2000);
  const sorted = [...stepMs].sort((a, b) => a - b);
  expect(sorted[Math.floor(sorted.length / 2)] ?? 0).toBeLessThan(40);

  // In the background: nothing moves and nothing is written.
  layer.setFocused(false);
  await pass(500);
  const idle = { bytes: written, steps: layer.steps, commits: commits.length };
  await pass(30_000, 500);
  expect(layer.steps - idle.steps).toBe(0);
  expect(written - idle.bytes).toBe(0);
  expect(commits.length - idle.commits).toBe(0);
  expect(layer.running).toBe(false);
});
