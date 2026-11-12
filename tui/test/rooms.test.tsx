// The rooms: half-block art, the sprite files, the grid of the cards, the
// fallbacks to the list, and an animation that rests while the terminal
// is in the background.
import { afterEach, describe, expect, test } from 'vitest';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { cleanup } from 'ink-testing-library';
import { detectArt, diffCells, drawChanges, drawRuns, halfBlock, prepareSprite, rgbTo256, rowText, type Cell } from '../src/art.js';
import { ArtLayer, FPS } from '../src/art-layer.js';
import {
  ART_COLORS,
  ART_STATES,
  artState,
  colorOf,
  FALLBACK_SPRITE,
  parseSprite,
  SPRITE_FPS,
  spriteFor,
  spriteName,
  STATE_FRAMES,
  type ArtState,
  type Sprite,
} from '../src/sprites.js';
import { SPRITE_FILES } from '../src/sprite-files.js';
import { CARD_H, CARD_MIN_W, COMPACT_H, roomsGrid } from '../src/views/rooms.js';
import { cells } from '../src/clean.js';
import { fakeWorld, renderApp, sleep, until, type Rendered } from './helpers.js';

afterEach(() => cleanup());

const here = dirname(fileURLToPath(import.meta.url));
const SPRITES = join(here, '..', 'sprites');
const STATES: ArtState[] = ['work', 'wait', 'idle', 'sleep', 'blocked'];

const R: [number, number, number] = [255, 0, 0];
const G: [number, number, number] = [0, 255, 0];
const B: [number, number, number] = [0, 0, 255];

// 3 x 3 pixels, two frames: rows 0-1 make the first row of cells, row 2
// the second (no bottom pixel).
const KNOWN: Sprite = {
  w: 3,
  h: 3,
  frames: [
    [R, null, R, G, G, null, B, null, null],
    [R, null, R, G, B, null, B, null, null],
  ],
};

function strip(s: string): string {
  return s.replace(/\u001b\[[\d;]*m/g, '');
}

function art(mode: 'truecolor' | '256', columns: number, rows: number, sink: ((s: string) => void) | null = null): ArtLayer {
  return new ArtLayer({ mode, sink, screen: () => ({ columns, rows }) });
}

describe('half blocks', () => {
  test('a known sprite gives the exact cells', () => {
    const c: Cell[][] = halfBlock(KNOWN, 0);
    expect(c).toEqual([
      [
        { ch: '▀', fg: R, bg: G },
        { ch: '▄', fg: G, bg: null },
        { ch: '▀', fg: R, bg: null },
      ],
      [
        { ch: '▀', fg: B, bg: null },
        { ch: ' ', fg: null, bg: null },
        { ch: ' ', fg: null, bg: null },
      ],
    ]);
  });

  test('the cells as text: 24-bit and 256 colours, a code only where the colour changes', () => {
    const row = halfBlock(KNOWN, 0)[0] as Cell[];
    expect(rowText(row, 'truecolor')).toBe(
      '\u001b[38;2;255;0;0;48;2;0;255;0m▀' + '\u001b[38;2;0;255;0;49m▄' + '\u001b[38;2;255;0;0m▀' + '\u001b[39;49m',
    );
    expect(rowText(row, '256')).toBe('\u001b[38;5;196;48;5;46m▀' + '\u001b[38;5;46;49m▄' + '\u001b[38;5;196m▀' + '\u001b[39;49m');
    expect(rowText(row, 'none')).toBe('   ');
    expect(rgbTo256([0, 0, 0])).toBe(16);
    expect(rgbTo256([255, 255, 255])).toBe(231);
    expect(rgbTo256([128, 128, 128])).toBe(244);
  });

  test('an animation step draws only the changed cells, and puts the cursor back', () => {
    const changes = diffCells(halfBlock(KNOWN, 0), halfBlock(KNOWN, 1));
    // Frame 2 changes one pixel: the bottom of the middle cell, row 0.
    expect(changes).toEqual([{ x: 1, y: 0, cell: { ch: '▄', fg: B, bg: null } }]);
    expect(drawChanges(changes, { x: 10, y: 4 }, 'truecolor')).toBe('\u001b7\u001b[5;12H\u001b[38;2;0;0;255m▄\u001b[39;49m\u001b[0m\u001b8');
    expect(drawChanges([], { x: 0, y: 0 }, 'truecolor')).toBe('');
  });
});

describe('sprite files', () => {
  test('the gray sprite of each state, 24 x 24, in the file format', () => {
    for (const s of STATES) {
      const text = readFileSync(join(SPRITES, `character-gray-${s}.json`), 'utf8');
      const sprite = parseSprite(text);
      expect(sprite, s).not.toBeNull();
      expect(sprite?.w).toBe(24);
      expect(sprite?.h).toBe(24);
      expect(sprite?.frames.length).toBeGreaterThanOrEqual(2);
    }
  });

  test('the build bundles every sprite file into the program', () => {
    const files = readdirSync(SPRITES).filter((f) => /^character-[a-z]+-[a-z]+\.json$/.test(f));
    expect(files.length).toBeGreaterThanOrEqual(5);
    for (const f of files) {
      const name = f.replace(/\.json$/, '');
      expect(SPRITE_FILES[name], name).toBeDefined();
      expect(JSON.parse(SPRITE_FILES[name] as string)).toEqual(JSON.parse(readFileSync(join(SPRITES, f), 'utf8')));
    }
  });

  test('the loader takes flat or row-by-row frames and refuses a wrong shape', () => {
    expect(parseSprite(JSON.stringify(KNOWN))).toEqual(KNOWN);
    const rows = { w: 3, h: 3, frames: KNOWN.frames.map((f) => [f.slice(0, 3), f.slice(3, 6), f.slice(6, 9)]) };
    expect(parseSprite(JSON.stringify(rows))).toEqual(KNOWN);
    expect(parseSprite('{')).toBeNull();
    expect(parseSprite(JSON.stringify({ w: 3, h: 3, frames: [[R]] }))).toBeNull();
    expect(parseSprite(JSON.stringify({ w: 1, h: 1, frames: [[[256, 0, 0]]] }))).toBeNull();
    expect(parseSprite(JSON.stringify({ w: 1, h: 1, frames: [] }))).toBeNull();
  });

  test('each agent has its colour, an own file wins over the gray file, the built-in character covers a missing file', () => {
    expect(['claude', 'codex', 'gemini', 'copilot', 'grok', 'opencode', 'aider'].map(colorOf)).toEqual(['orange', 'blue', 'purple', 'green', 'pink', 'teal', 'gray']);
    const loop = (n: number, rgb: [number, number, number]): string =>
      JSON.stringify({ w: 24, h: 24, fps: 8, frames: Array.from({ length: n }, () => Array.from({ length: 24 * 24 }, () => rgb)) });
    const gray = loop(4, [128, 128, 128]);
    const own = loop(4, [9, 9, 9]);
    const files = { 'character-gray-sleep': gray };
    // No own file: the gray art of the state as it is, no tint.
    expect(spriteFor('orange', 'sleep', files)?.frames[0]?.[0]).toEqual([128, 128, 128]);
    expect(spriteFor('orange', 'sleep', { ...files, 'character-orange-sleep': own })?.frames[0]?.[0]).toEqual([9, 9, 9]);
    // No file at all, a broken file, the wrong size, frame count or rate: the built-in character.
    expect(spriteFor('orange', 'idle', files)).toBe(FALLBACK_SPRITE);
    expect(spriteFor('gray', 'idle', { 'character-gray-idle': '{' })).toBe(FALLBACK_SPRITE);
    expect(spriteFor('blue', 'sleep', { 'character-blue-sleep': loop(8, [1, 2, 3]) })).toBe(FALLBACK_SPRITE);
    expect(spriteFor('blue', 'sleep', { 'character-blue-sleep': own.replace('"fps":8', '"fps":12') })).toBe(FALLBACK_SPRITE);
    expect(spriteFor('blue', 'sleep', { 'character-blue-sleep': JSON.stringify({ w: 2, h: 2, frames: [[null, null, null, null]] }) })).toBe(FALLBACK_SPRITE);
    // The built-in character: 24 x 24, one still frame, the ink outline of the art set.
    expect(FALLBACK_SPRITE).toMatchObject({ w: 24, h: 24 });
    expect(FALLBACK_SPRITE.frames).toHaveLength(1);
    expect(FALLBACK_SPRITE.frames[0]).toHaveLength(24 * 24);
    expect(FALLBACK_SPRITE.frames[0]?.some((p) => p && p[0] === 43 && p[1] === 33 && p[2] === 64)).toBe(true);
    // The bundled art: claude is the orange toon, not the fallback and not the gray one.
    const claude = spriteFor('orange', 'work');
    expect(claude).not.toBe(FALLBACK_SPRITE);
    expect(claude).not.toEqual(spriteFor('gray', 'work'));
    expect(claude?.frames[0]?.some((p) => p && p[0] > 200 && p[2] < 120)).toBe(true);
  });

  test('every colour and state loads its own file from the bundle', () => {
    expect(ART_COLORS).toHaveLength(7);
    expect(ART_STATES).toHaveLength(5);
    for (const color of ART_COLORS) {
      for (const state of ART_STATES) {
        const name = spriteName(color, state);
        const s = spriteFor(color, state);
        expect(s, name).not.toBe(FALLBACK_SPRITE);
        expect(s, name).toEqual(parseSprite(readFileSync(join(SPRITES, `${name}.json`), 'utf8')));
        // Kept: the second call gives the same sprite, parsed once.
        expect(spriteFor(color, state), name).toBe(s);
      }
    }
  });

  test('the bundle holds only the files that the interface uses', () => {
    const want = ART_COLORS.flatMap((c) => ART_STATES.map((s) => spriteName(c, s))).sort();
    expect(Object.keys(SPRITE_FILES).sort()).toEqual(want);
    expect(want).toHaveLength(35);
  });

  test('the frame counts and the rate are those of sprites/frames.json', () => {
    const spec = JSON.parse(readFileSync(join(SPRITES, 'frames.json'), 'utf8')) as { fps: number; frames: Record<string, number> };
    expect(spec.fps).toBe(8);
    const table: Record<string, number> = spec.frames;
    expect(table).toEqual({ idle: 8, work: 8, wait: 8, sleep: 4, blocked: 4 });
    expect(STATE_FRAMES).toEqual(table);
    expect(SPRITE_FPS).toBe(8);
    expect(FPS).toBe(8);
    for (const color of ART_COLORS) {
      for (const state of ART_STATES) {
        const name = spriteName(color, state);
        const file = JSON.parse(readFileSync(join(SPRITES, `${name}.json`), 'utf8')) as { w: number; h: number; fps: number; frames: unknown[] };
        expect([file.w, file.h, file.fps, file.frames.length], name).toEqual([24, 24, 8, table[state]]);
        expect(spriteFor(color, state).frames, name).toHaveLength(table[state] as number);
      }
    }
  });

  test('the clock plays every loop at 8 fps: the frame follows the tick', () => {
    const layer = art('truecolor', 80, 24);
    const idle = spriteFor('orange', 'idle');
    const sleep = spriteFor('orange', 'sleep');
    const seen: [number, number][] = [];
    for (let t = 0; t < 10; t++) {
      layer.tick = t;
      seen.push([layer.frameOf(idle), layer.frameOf(sleep)]);
    }
    expect(seen).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9].map((t) => [t % 8, t % 4]));
  });

  test('the cells and codes are made once per sprite, and a step uses the ready runs', () => {
    const s = spriteFor('teal', 'wait');
    const p = prepareSprite(s, 'truecolor');
    expect(p.cells).toHaveLength(8);
    expect(p.rows).toHaveLength(8);
    for (let f = 0; f < 8; f++) {
      expect(p.cells[f]).toEqual(halfBlock(s, f));
      expect(p.rows[f]).toEqual(halfBlock(s, f).map((r) => rowText(r, 'truecolor')));
      const to = (f + 1) % 8;
      const at = { x: 3, y: 2 };
      expect(drawRuns(p.step(f, to), at)).toBe(drawChanges(diffCells(halfBlock(s, f), halfBlock(s, to)), at, 'truecolor'));
      // The same runs every time: made once, not per frame.
      expect(p.step(f, to)).toBe(p.step(f, to));
    }
    expect(drawRuns(p.step(2, 6), { x: 0, y: 0 })).toBe(drawChanges(diffCells(halfBlock(s, 2), halfBlock(s, 6)), { x: 0, y: 0 }, 'truecolor'));
    const layer = art('256', 80, 24);
    expect(layer.prepared(s)).toBe(layer.prepared(s));
    expect(layer.rowsOf(s, 9)).toBe(layer.prepared(s).rows[1]);
  });

  test('a real character in 24-bit colour and in the 256 colours', () => {
    const s = spriteFor('orange', 'idle');
    expect(prepareSprite(s, 'truecolor').rows[0]?.join('\n')).toMatchSnapshot('claude idle frame 0, truecolor');
    expect(prepareSprite(s, '256').rows[0]?.join('\n')).toMatchSnapshot('claude idle frame 0, 256 colours');
    // The picture without the codes: 12 rows of 24 cells.
    for (const r of prepareSprite(s, '256').rows[0] ?? []) expect(cells(strip(r))).toBe(24);
    expect(prepareSprite(s, '256').rows[0]?.join('')).not.toMatch(/\u001b\[(38|48);2;/);
  });

  test('the states of paddock map to the sprite states', () => {
    const of = (state: Parameters<typeof artState>[0]['state'], frozen = false) => artState({ state, frozen });
    expect(of('running')).toBe('work');
    expect(of('waiting')).toBe('wait');
    expect(of('idle')).toBe('idle');
    expect(of('done')).toBe('sleep');
    expect(of('ended')).toBe('sleep');
    expect(of('running', true)).toBe('sleep');
    expect(of('failed')).toBe('blocked');
    expect(of('blocked')).toBe('blocked');
  });
});

describe('the terminal decides the art', () => {
  const tty = true;
  test.each([
    [{ TERM: 'xterm-256color', COLORTERM: 'truecolor' }, 'truecolor'],
    [{ TERM: 'xterm-256color' }, '256'],
    [{ TERM: 'xterm-kitty' }, 'truecolor'],
    [{ TERM: 'xterm' }, 'none'],
    [{ TERM: 'dumb' }, 'none'],
    [{ TERM: 'xterm-256color', COLORTERM: 'truecolor', NO_COLOR: '1' }, 'none'],
    [{ TERM: 'xterm-256color', FORCE_COLOR: '0' }, 'none'],
    [{ TERM: 'xterm-256color', FORCE_COLOR: '1' }, 'none'],
    [{ TERM: 'xterm', FORCE_COLOR: '3' }, 'truecolor'],
    [{ TERM: 'xterm-256color', PADDOCK_TUI_ART: 'off' }, 'none'],
    [{ TERM: 'xterm', PADDOCK_TUI_ART: '256' }, '256'],
  ] as [NodeJS.ProcessEnv, string][])('%j: %s', (env, mode) => {
    expect(detectArt(env, tty)).toBe(mode);
  });

  test('no terminal or --plain: no art', () => {
    expect(detectArt({ TERM: 'xterm-256color', COLORTERM: 'truecolor' }, false)).toBe('none');
    expect(detectArt({ TERM: 'xterm-256color', COLORTERM: 'truecolor' }, true, true)).toBe('none');
  });
});

/** The rows of cards: lines with a card's top border, and the cards on each. */
function cardRows(r: Rendered): number[] {
  return r
    .lines()
    .map(strip)
    .filter((l) => l.includes('╭─ '))
    .map((l) => l.split('╭─ ').length - 1);
}

function fits(r: Rendered, columns: number, rows: number): void {
  const lines = r.lines();
  expect(lines.length).toBeLessThanOrEqual(rows);
  for (const l of lines) expect(cells(strip(l)), strip(l)).toBeLessThanOrEqual(columns);
}

describe('the grid', () => {
  test.each([
    [80, 2, 40],
    [120, 4, 30],
    [200, 7, 28],
  ])('%i columns: %i cards in a row', (columns, cols, width) => {
    const g = roomsGrid(columns, 40, true);
    expect(g.cols).toBe(cols);
    expect(Math.min(...g.widths)).toBeGreaterThanOrEqual(CARD_MIN_W);
    expect(g.widths[g.widths.length - 1]).toBe(width);
    expect(g.widths.reduce((a, b) => a + b, 0)).toBe(columns);
    expect(g.cardH).toBe(CARD_H);
    expect(g.rows).toBe(2);
    // Too low for the art: compact cards with the words only.
    expect(roomsGrid(columns, CARD_H - 1, true)).toMatchObject({ art: false, cardH: COMPACT_H });
  });

  test.each([
    [80, 24, [2]],
    [120, 34, [4]],
    [200, 60, [7, 0]],
  ])('%ix%i: the runs as rooms, each with its words and its character', async (columns, rows, expected) => {
    fakeWorld();
    const r = await renderApp({ columns, rows, art: art('truecolor', columns, rows) });
    const f = await r.waitFor((x) => x.includes('◆ waits for you') && x.includes('←→↑↓ move'));
    const plain = strip(f);
    const got = cardRows(r);
    // The approval strip on a wide screen has a top border too.
    const cards = columns >= 120 ? got.slice(1) : got;
    expect(cards[0]).toBe(expected[0]);
    if (columns === 200) expect(cards.length).toBe(1);
    // The words of the list are on the cards: state, time in it, agent, project, place.
    expect(plain).toMatch(/◆ waits for you · \d+ s/);
    expect(plain).toContain('codex · alpha');
    expect(plain).toContain('sandbox bwrap');
    expect(plain).toContain('Runs · 4 live, 3 ended');
    // The art: half blocks in 24-bit colour.
    expect(f).toContain('▀');
    expect(f).toMatch(/\u001b\[38;2;\d+;\d+;\d+/);
    fits(r, columns, rows);
  });

  test('200x60 shows every run, the arrows move by card and by row', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 200, rows: 60, art: art('truecolor', 200, 60) });
    await r.waitFor('1/7');
    for (const w of ['◆ waits for you', '▲ stuck', '○ idle', '✓ done', '✖ failed', '✓ ended']) expect(strip(r.frame())).toContain(w);
    await r.press('\u001b[C');
    await r.waitFor('2/7');
    await r.press('\u001b[B');
    await r.waitFor((f) => strip(f).includes('7/7'));
    await r.press('\u001b[A');
    await r.waitFor('1/7');
  });

  test('Enter opens the selected room', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 120, rows: 34, art: art('truecolor', 120, 34) });
    await r.waitFor('1/7');
    await r.press('\u001b[C');
    await r.waitFor('2/7');
    await r.press('\r');
    await r.waitFor((f) => strip(f).includes('Run · codex-0b0b0b · sandbox bwrap'));
  });
});

describe('fallbacks', () => {
  test('under 80 columns: the list', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 79, rows: 24, art: art('truecolor', 79, 24) });
    const f = strip(await r.waitFor('✖ failed'));
    expect(f).not.toContain('←→↑↓ move');
    expect(f).toContain('sandbox seatbelt ');
    expect(r.frame()).not.toContain('▀');
  });

  test('--plain: the list, no art', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 120, rows: 34, plain: true, mouse: false, art: art('truecolor', 120, 34) });
    const f = await r.waitFor('✖ failed');
    expect(f).not.toContain('←→↑↓ move');
    expect(f).not.toContain('▀');
  });

  test('NO_COLOR: no art, the list', async () => {
    fakeWorld();
    const mode = detectArt({ TERM: 'xterm-256color', COLORTERM: 'truecolor', NO_COLOR: '1' }, true);
    const r = await renderApp({ columns: 120, rows: 34, art: new ArtLayer({ mode, sink: () => undefined, screen: () => ({ columns: 120, rows: 34 }) }) });
    const f = await r.waitFor('✖ failed');
    expect(f).not.toContain('←→↑↓ move');
    expect(f).not.toContain('▀');
    expect(f).toContain('sandbox seatbelt ');
  });

  test('256 colours: the art in the 256-colour codes, no 24-bit code', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 80, rows: 24, art: art('256', 80, 24) });
    const f = await r.waitFor('1/7');
    expect(f).toContain('▀');
    expect(f).toMatch(/\u001b\[(38|48);5;\d+/);
    expect(f).not.toMatch(/\u001b\[(38|48);2;/);
  });

  test('L switches between the rooms and the list', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 100, rows: 30, art: art('truecolor', 100, 30) });
    await r.waitFor('1/7');
    await r.press('L');
    const f = await r.waitFor((x) => x.includes('Runs as a list') && x.includes('✖ failed'));
    expect(f).not.toContain('←→↑↓ move');
    expect(strip(f)).toContain('sandbox seatbelt ');
    await r.press('L');
    await r.waitFor('1/7');
  });
});

describe('animation', () => {
  test('8 frames a second with the focus, only the changed cells, nothing while the terminal is in the background', async () => {
    fakeWorld();
    const writes: string[] = [];
    const layer = art('truecolor', 80, 24, (s) => writes.push(s));
    const r = await renderApp({ columns: 80, rows: 24, art: layer });
    await r.waitFor('1/7');
    await until(() => layer.running && writes.length >= 2);
    // Each step jumps to the changed cells and draws only those: far less than a sprite.
    for (const w of writes) {
      expect(w.startsWith('\u001b7')).toBe(true);
      expect(w.endsWith('\u001b8')).toBe(true);
      expect((w.match(/[▀▄ ]/g) ?? []).length).toBeLessThan(24 * 12);
    }
    // Focus out (ESC [ O): the clock stops, no frame more.
    await r.press('\u001b[O');
    expect(layer.running).toBe(false);
    const n = writes.length;
    const steps = layer.steps;
    await sleep(600);
    expect(writes.length).toBe(n);
    expect(layer.steps).toBe(steps);
    // Focus in (ESC [ I): it goes on. Neither report is a key.
    await r.press('\u001b[I');
    expect(layer.running).toBe(true);
    await until(() => writes.length > n);
    expect(strip(r.frame())).not.toContain('Runs as a list');
  });

  test('no clock without a sprite on the screen or without a terminal to write to', async () => {
    fakeWorld();
    const writes: string[] = [];
    const layer = art('truecolor', 80, 24, (s) => writes.push(s));
    const r = await renderApp({ columns: 80, rows: 24, art: layer });
    await r.waitFor('1/7');
    await until(() => layer.running);
    // The help covers the rooms: their sprites go, and so does the clock.
    await r.press('?');
    await r.waitFor('Help · every key');
    await until(() => !layer.running);
    const n = writes.length;
    await sleep(400);
    expect(writes.length).toBe(n);
    const still = art('truecolor', 80, 24, null);
    const r2 = await renderApp({ columns: 80, rows: 24, art: still });
    await r2.waitFor('1/7');
    await sleep(300);
    expect(still.running).toBe(false);
    expect(still.steps).toBe(0);
  });
});
