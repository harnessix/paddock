// The painter: a real terminal (xterm) gets only the cells that change, and
// always shows exactly the frame of Ink, with the sprites of the art layer
// on top. No frame clears the screen, no line is written again without a
// change.
import { afterEach, describe, expect, test, vi } from 'vitest';
import xterm from '@xterm/headless';
import { cleanup } from 'ink-testing-library';
import { blankRow, blankStyle, cellStyle, Painter, parseLine, SYNC_OFF, SYNC_ON, type Overlay, type Row } from '../src/screen.js';
import { ArtLayer } from '../src/art-layer.js';
import { prepareSprite, rowText } from '../src/art.js';
import { spriteFor } from '../src/sprites.js';
import { fakeWorld, find, renderApp, until } from './helpers.js';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function strip(s: string): string {
  return s.replace(/\u001b\[[\d;]*m/g, '');
}

interface Screen {
  term: xterm.Terminal;
  painter: Painter;
  writes: string[];
  /** Wait until xterm took every write. */
  settle: () => Promise<void>;
  /** The cells that xterm shows, as the painter keeps cells. */
  row: (y: number) => Row;
  size: { columns: number; rows: number };
}

function screen(columns: number, rows: number, opts: { overlays?: () => Overlay[]; minFrameMs?: number; now?: () => number; ready?: () => boolean } = {}): Screen {
  const term = new xterm.Terminal({ cols: columns, rows, allowProposedApi: true });
  const writes: string[] = [];
  const size = { columns, rows };
  const painter = new Painter({
    write: (s) => {
      writes.push(s);
      term.write(s);
      return opts.ready ? opts.ready() : true;
    },
    size: () => size,
    overlays: opts.overlays,
    minFrameMs: opts.minFrameMs ?? 0,
    now: opts.now,
  });
  const cell = term.buffer.active.getNullCell();
  const color = (fg: boolean): string => {
    const v = fg ? cell.getFgColor() : cell.getBgColor();
    const base = fg ? 30 : 40;
    if (fg ? cell.isFgRGB() : cell.isBgRGB()) return `${base + 8};2;${(v >> 16) & 255};${(v >> 8) & 255};${v & 255}`;
    if (fg ? cell.isFgPalette() : cell.isBgPalette()) return v < 8 ? String(base + v) : v < 16 ? String(base + 60 + v - 8) : `${base + 8};5;${v}`;
    return '';
  };
  return {
    term,
    painter,
    writes,
    size,
    settle: () => new Promise<void>((r) => term.write('', r)),
    row: (y) => {
      const line = term.buffer.active.getLine(y);
      const out = blankRow(term.cols);
      for (let x = 0; x < term.cols; x++) {
        line?.getCell(x, cell);
        if (cell.getWidth() === 0) {
          out.ch[x] = '';
          out.st[x] = out.st[x - 1] ?? '';
          continue;
        }
        const ch = cell.getChars() || ' ';
        const attrs = [cell.isBold() ? '1' : '', cell.isDim() ? '2' : '', cell.isItalic() ? '3' : '', cell.isUnderline() ? '4' : '', cell.isInverse() ? '7' : ''].filter((a) => a);
        const plain = ch === ' ' && !cell.isUnderline() && !cell.isInverse();
        out.ch[x] = ch;
        out.st[x] = plain ? color(false) : [...attrs, color(true), color(false)].filter((a) => a).join(';');
      }
      return out;
    },
  };
}

/** The terminal shows the frame, cell by cell: characters and styles. */
async function expectShows(s: Screen, frame: string, overlays: Overlay[] = []): Promise<void> {
  await s.settle();
  const lines = frame.split('\n');
  for (let y = 0; y < s.size.rows; y++) {
    const want = parseLine(lines[y] ?? '', s.size.columns);
    for (const o of overlays) {
      const src = o.rows[y - o.y];
      if (!src) continue;
      // A wide character of the frame that the edge of the overlay cuts shows as a blank.
      for (const at of [want.ch[o.x] === '' ? o.x - 1 : -1, want.ch[o.x + src.ch.length] === '' ? o.x + src.ch.length : -1]) {
        if (at < 0) continue;
        want.ch[at] = ' ';
        want.st[at] = blankStyle(want.st[at] as string);
      }
      src.ch.forEach((ch, dx) => ((want.ch[o.x + dx] = ch), (want.st[o.x + dx] = src.st[dx] as string)));
    }
    const got = s.row(y);
    expect(got.ch.join(''), `row ${y}`).toBe(want.ch.join(''));
    expect(got.st, `styles of row ${y}: ${want.ch.join('')}`).toEqual(want.st);
  }
}

describe('a line of a frame as cells', () => {
  test('characters and styles, in a fixed order of the codes', () => {
    const r = parseLine('a\u001b[1mb\u001b[31;44mc\u001b[22;39;49m d\u001b[38;5;196;48;2;1;2;3m▀\u001b[0m', 8);
    expect(r.ch).toEqual(['a', 'b', 'c', ' ', 'd', '▀', ' ', ' ']);
    expect(r.st).toEqual(['', '1', '1;31;44', '', '', '38;5;196;48;2;1;2;3', '', '']);
  });

  test('a blank cell keeps only what a space shows', () => {
    const r = parseLine('\u001b[1;31m \u001b[44m \u001b[7m \u001b[0m\u001b[4m \u001b[0m', 4);
    expect(r.st).toEqual(['', '44', '1;7;31;44', '4']);
    expect(cellStyle(' ', '31', '44')).toBe('44');
    expect(cellStyle('▀', '31', '44')).toBe('31;44');
    expect(cellStyle('▄', '31', '')).toBe('31');
  });

  test('a wide character takes two cells, a mark joins its letter, the line is cut at the width', () => {
    const r = parseLine('a日b éx', 7);
    expect(r.ch).toEqual(['a', '日', '', 'b', ' ', 'é', 'x']);
    expect(parseLine('abc日', 4).ch).toEqual(['a', 'b', 'c', ' ']);
    expect(parseLine('abcdef', 3).ch).toEqual(['a', 'b', 'c']);
  });

  test('other escape sequences are not cells', () => {
    expect(parseLine('a\u001b[2Kb\u001b]8;;http://x\u0007c\u001b]8;;\u0007', 4).ch).toEqual(['a', 'b', 'c', ' ']);
  });
});

describe('the painter writes only the cells that change', () => {
  test('the first frame clears once, the same frame writes nothing, one changed cell is one cell', async () => {
    const s = screen(20, 4);
    const a = 'paddock · 3 running\n\u001b[1mRuns\u001b[22m  Board\n\nfooter';
    s.painter.frame(a);
    await expectShows(s, a);
    expect(s.painter.stats.clears).toBe(1);
    expect(s.writes).toHaveLength(1);
    expect(s.writes[0]?.startsWith(SYNC_ON)).toBe(true);
    expect(s.writes[0]?.endsWith(SYNC_OFF)).toBe(true);
    s.painter.frame(a);
    expect(s.writes).toHaveLength(1);
    const cells = s.painter.stats.cells;
    const b = a.replace('3 running', '4 running');
    s.painter.frame(b);
    await expectShows(s, b);
    expect(s.painter.stats.cells - cells).toBe(1);
    // The cursor jumps to the cell: row 1, column 11. Nothing is erased.
    expect(s.writes[1]).toBe(`${SYNC_ON}\u001b[1;11H\u001b[0m4\u001b[0m${SYNC_OFF}`);
    expect(s.writes.slice(1).join('')).not.toMatch(/\u001b\[[\d;]*[JK]/);
    expect(s.painter.stats.clears).toBe(1);
  });

  test('changes that lie close are one run, changes far apart are two', () => {
    const s = screen(40, 1);
    s.painter.frame('aaaa bbbb cccc dddd eeee ffff gggg');
    s.painter.frame('aaXa bXbb cccc dddd eeee ffff gYgg');
    expect(s.writes[1]).toBe(`${SYNC_ON}\u001b[1;3H\u001b[0mXa bX\u001b[1;32HY\u001b[0m${SYNC_OFF}`);
  });

  test('a style changes with the shortest code: only the colour that differs', () => {
    const s = screen(10, 1);
    s.painter.frame('');
    s.painter.frame('\u001b[38;5;1;48;5;2m▀\u001b[38;5;3m▀\u001b[48;5;4m▀\u001b[49m \u001b[38;5;3m▄\u001b[0m');
    // The blank cell keeps the foreground, so the cell after it needs no code.
    expect(s.writes[1]).toBe(`${SYNC_ON}\u001b[1;1H\u001b[0;38;5;1;48;5;2m▀\u001b[38;5;3m▀\u001b[48;5;4m▀\u001b[49m ▄\u001b[0m${SYNC_OFF}`);
  });

  test('wide characters are written whole when a neighbour changes', async () => {
    const s = screen(12, 2);
    const frames = ['ab日本cd\nxx', 'abXY本cd\n日x', 'a日本本cd\nxx', '日本語日本語\n', 'abcdefghijkl\n日本'];
    for (const f of frames) {
      s.painter.frame(f);
      await expectShows(s, f);
    }
  });

  test('a new size clears once and draws the frame in it', async () => {
    const s = screen(20, 4);
    s.painter.frame('one\ntwo\nthree\nfour');
    s.size.columns = 10;
    s.size.rows = 3;
    s.term.resize(10, 3);
    s.painter.frame('one\ntwo\nthree');
    await expectShows(s, 'one\ntwo\nthree');
    expect(s.painter.stats.clears).toBe(2);
    // A frame longer than the screen is cut, not scrolled.
    s.painter.frame('1\n2\n3\n4\n5');
    await expectShows(s, '1\n2\n3');
    expect(s.term.buffer.active.baseY).toBe(0);
  });

  test('on request, every cell is drawn again: the screen is right after another program wrote over it', async () => {
    const s = screen(20, 3);
    const f = 'one\n\u001b[1mtwo\u001b[22m\nthree';
    s.painter.frame(f);
    await s.settle();
    // Another program writes over the screen. The painter does not know.
    s.term.write('\u001b[1;1HGARBAGE\u001b[3;1HMORE');
    s.painter.frame(f);
    await s.settle();
    expect(s.row(0).ch.join('').trim()).toBe('GARBAGE');
    s.painter.invalidate();
    await expectShows(s, f);
    expect(s.painter.stats.clears).toBe(2);
  });

  test('frames that come too fast are joined: the first goes at once, then only the last', () => {
    vi.useFakeTimers();
    let now = 1000;
    const s = screen(10, 1, { minFrameMs: 16, now: () => now });
    s.painter.frame('a');
    expect(s.writes).toHaveLength(1);
    now += 2;
    s.painter.frame('b');
    s.painter.frame('c');
    s.painter.frame('d');
    expect(s.writes).toHaveLength(1);
    now += 14;
    vi.advanceTimersByTime(14);
    expect(s.writes).toHaveLength(2);
    expect(s.writes[1]).toContain('d');
    expect(s.painter.text()).toEqual(['d']);
    expect(s.painter.stats).toMatchObject({ frames: 4, painted: 2 });
    // After a pause, a frame goes at once again: a key gets its answer without a wait.
    now += 100;
    s.painter.frame('e');
    expect(s.writes).toHaveLength(3);
  });

  test('a terminal that is not ready gets no more frames until it is, then the last one', () => {
    let ready = false;
    const s = screen(10, 1, { ready: () => ready });
    s.painter.frame('a');
    s.painter.frame('b');
    s.painter.frame('c');
    expect(s.writes).toHaveLength(1);
    expect(s.painter.patch({ x: 0, y: 0, rows: [parseLine('z', 1)] })).toBe(false);
    ready = true;
    s.painter.drained();
    expect(s.writes).toHaveLength(2);
    expect(s.painter.text()).toEqual(['c']);
  });

  test('after stop, nothing is written', () => {
    const s = screen(10, 1);
    s.painter.frame('a');
    s.painter.stop();
    s.painter.frame('b');
    expect(s.painter.patch({ x: 0, y: 0, rows: [parseLine('z', 1)] })).toBe(false);
    expect(s.writes).toHaveLength(1);
  });
});

describe('any frame after any frame', () => {
  test('random frames with styles, wide characters and patches: the terminal always shows the last one', async () => {
    // A fixed seed: the same frames in every run.
    let seed = 0x2f6e2b1;
    const rnd = (n: number): number => {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      return seed % n;
    };
    const chars = ['a', 'b', 'x', ' ', ' ', '日', '本', '▀', '▄', '│', 'é', '◆'];
    const styles = ['', '', '1', '2', '4', '7', '31', '32;44', '1;33', '38;5;200', '48;5;17', '38;2;10;20;30;48;2;200;100;50', '2;7;36'];
    const columns = 30;
    const rows = 6;
    const line = (): string => {
      let out = '';
      const n = rnd(columns + 6);
      for (let i = 0; i < n; i++) {
        const st = styles[rnd(styles.length)] as string;
        out += (st ? `\u001b[${st}m` : '') + (chars[rnd(chars.length)] as string) + (st && rnd(3) > 0 ? '\u001b[0m' : '');
      }
      return out + '\u001b[0m';
    };
    let over: Overlay[] = [];
    const s = screen(columns, rows, { overlays: () => over });
    let last = Array.from({ length: rows }, line);
    for (let i = 0; i < 120; i++) {
      // Most frames change a line or two, some change all, some none.
      const kind = rnd(10);
      if (kind < 6) last = last.map((l) => (rnd(3) === 0 ? line() : l));
      else if (kind < 8) last = Array.from({ length: rows }, line);
      if (rnd(4) === 0) over = rnd(2) === 0 ? [] : [{ x: rnd(columns - 4), y: rnd(rows - 1), rows: [parseLine(line(), 4), parseLine(line(), 4)] }];
      // A wide character must not be cut by the edge of an overlay: the art has none, and neither have these.
      for (const o of over) for (const r of o.rows) r.ch.forEach((c, j) => ((c === '' || (c !== ' ' && r.ch[j + 1] === '')) && ((r.ch[j] = ' '), (r.st[j] = ''))));
      const f = last.join('\n');
      s.painter.frame(f);
      if (rnd(3) === 0 && over[0]) {
        // The next frame of a sprite: the same place and size, other cells.
        const o: Overlay = { ...over[0], rows: [parseLine('\u001b[38;5;99m▀▀▄ \u001b[0m', 4), over[0].rows[1] as Row] };
        over = [o];
        s.painter.patch(o);
      }
      await expectShows(s, f, over);
    }
    expect(s.painter.stats.clears).toBe(1);
  });
});

describe('sprites over the frame', () => {
  test('the cells of a sprite are the cells of its text', () => {
    for (const mode of ['truecolor', '256'] as const) {
      const sprite = spriteFor('orange', 'wait');
      const p = prepareSprite(sprite, mode);
      p.cells.forEach((frame, f) => frame.forEach((cells, y) => expect(p.screen[f]?.[y], `${mode} frame ${f} row ${y}`).toEqual(parseLine(rowText(cells, mode), sprite.w))));
    }
  });

  const A: Row = parseLine('\u001b[38;5;201m▀▀\u001b[38;5;202m▄\u001b[0m', 3);
  const B: Row = parseLine('\u001b[38;5;201m▀\u001b[38;5;204m▀\u001b[38;5;202m▄\u001b[0m', 3);

  test('an overlay is part of the frame, and a patch writes only its changed cells', async () => {
    let over: Overlay = { x: 2, y: 1, rows: [A] };
    const s = screen(10, 3, { overlays: () => [over] });
    const f = 'head\n|      |\nfoot';
    s.painter.frame(f);
    await expectShows(s, f, [over]);
    // The next frame of the sprite: one cell differs.
    over = { x: 2, y: 1, rows: [B] };
    const cells = s.painter.stats.cells;
    expect(s.painter.patch(over)).toBe(true);
    expect(s.painter.stats.cells - cells).toBe(1);
    // The cursor is saved and put back around the patch.
    expect(s.writes[1]).toBe('\u001b7\u001b[2;4H\u001b[0;38;5;204m▀\u001b[0m\u001b8');
    await expectShows(s, f, [over]);
    // The same cells again: nothing to write.
    expect(s.painter.patch(over)).toBe(false);
    // Ink draws the same frame again, or another line of it: the sprite is not written again.
    s.painter.frame(f);
    s.painter.frame(f.replace('foot', 'feet'));
    expect(s.writes).toHaveLength(3);
    expect(s.writes[2]).not.toContain('▀');
    await expectShows(s, f.replace('foot', 'feet'), [over]);
  });

  test('a patch comes after the frame that waits', () => {
    vi.useFakeTimers();
    let now = 0;
    const s = screen(10, 2, { minFrameMs: 16, now: () => now });
    s.painter.frame('one\n');
    now += 1;
    s.painter.frame('two\n');
    expect(s.painter.text()[0]).toBe('one');
    s.painter.patch({ x: 0, y: 1, rows: [A] });
    expect(s.painter.text()).toEqual(['two', '▀▀▄']);
    // The frame went with the patch: the timer has nothing left to write.
    const n = s.writes.length;
    vi.advanceTimersByTime(100);
    expect(s.writes.length).toBe(n);
  });
});

describe('the interface through the painter', () => {
  test('every screen of the interface shows in the terminal as Ink drew it, cell by cell, without a second clear', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 100, rows: 30 });
    const s = screen(100, 30);
    const show = async (): Promise<void> => {
      s.painter.frame(r.frame());
      await expectShows(s, r.frame());
    };
    await r.waitFor('sandbox bwrap');
    await show();
    const steps: [string, string][] = [
      ['a', '[1] Allow once'],
      ['\u001b[C', '[1] Allow once'],
      ['b', 'Board · projects'],
      ['m', 'Mail · '],
      ['?', 'Help · every key'],
      ['\u001b', 'Mail · '],
      [':', 'Command palette'],
      ['\u001b', 'Mail · '],
      ['r', 'Runs · 4 live'],
      ['\r', 'Output · last'],
      ['\u001b', 'Runs · 4 live'],
      ['/', '↵ keep'],
      ['codex', 'filter "codex"'],
    ];
    for (const [keys, text] of steps) {
      await r.press(keys);
      await r.waitFor(text);
      await show();
    }
    expect(s.painter.stats.clears).toBe(1);
    expect(s.writes.slice(1).join('')).not.toMatch(/\u001b\[[\d;]*[JK]/);
  });

  test('moving the selection by one row writes two rows of cells, not the screen', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 100, rows: 30 });
    const s = screen(100, 30);
    await r.waitFor((f) => f.includes('sandbox bwrap') && f.includes('no paddock split'));
    s.painter.frame(r.frame());
    const before = s.painter.stats.cells;
    const frames = s.writes.length;
    await r.press('\u001b[B');
    await r.waitFor((f) => find(strip(f), '› ').y === 3);
    s.painter.frame(r.frame());
    await expectShows(s, r.frame());
    expect(s.writes.length).toBe(frames + 1);
    expect(s.painter.stats.cells - before).toBeLessThanOrEqual(2 * 100);
    const rows = new Set([...(s.writes[frames] as string).matchAll(/\u001b\[(\d+);\d+H/g)].map((m) => m[1]));
    expect([...rows].sort()).toEqual(['3', '4']);
  });

  test('the rooms: the art layer draws its steps through the painter, and the terminal shows the frame of the clock', async () => {
    fakeWorld();
    const s = screen(120, 34, { overlays: () => layer.overlays() });
    // A clock that never ticks by itself: the test makes the steps.
    const layer: ArtLayer = new ArtLayer({ mode: 'truecolor', sink: null, screen: () => s.size, painter: s.painter, fps: 0.001 });
    const r = await renderApp({ columns: 120, rows: 34, art: layer });
    await r.waitFor((f) => f.includes('1/7') && f.includes('no paddock split'));
    await until(() => layer.overlays().length === 4);
    // Ink keeps the place of each sprite free, and the painter puts the cells of the art layer there.
    const f = r.frame();
    expect(f).not.toContain('▀');
    s.painter.frame(f);
    const sprites = layer.overlays();
    await expectShows(s, f, sprites);
    expect(s.row((sprites[0]?.y ?? 0) + 6).ch.join('')).toContain('▀');
    const cells = s.painter.stats.cells;
    s.painter.frame(f);
    expect(s.painter.stats.cells).toBe(cells);
    // Steps of the animation: only patches, each far smaller than the sprites, and the screen follows the clock.
    const n = s.writes.length;
    for (let i = 0; i < 9; i++) {
      const tick = layer.tick;
      const c0 = s.painter.stats.cells;
      layer.step();
      expect(layer.tick).toBe(tick + 1);
      expect(s.painter.stats.cells - c0).toBeLessThan(4 * 24 * 12);
      await expectShows(s, f, layer.overlays());
    }
    for (const w of s.writes.slice(n)) {
      expect(w.startsWith('\u001b7\u001b[')).toBe(true);
      expect(w.endsWith('\u001b8')).toBe(true);
    }
    expect(layer.steps).toBe(9);
    // Ink draws again (the selection moves): the sprites stay at the frame of the clock.
    await r.press('\u001b[C');
    const f2 = await r.waitFor('2/7');
    s.painter.frame(f2);
    await expectShows(s, f2, layer.overlays());
    // In the background: no step, no write.
    layer.setFocused(false);
    const quiet = s.writes.length;
    layer.step();
    expect(s.writes.length).toBe(quiet);
    expect(layer.running).toBe(false);
  });
});
