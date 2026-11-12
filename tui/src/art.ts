// Half-block pixel art: two pixels in each cell. "▀" draws the top pixel
// in the foreground colour and the bottom pixel in the background colour.
// 24-bit colour where the terminal has it, else the 256 colours, else no
// art at all.

import type { Pixel, Rgb, Sprite } from './sprites.js';
import { cellStyle, type Row } from './screen.js';

export type ArtMode = 'truecolor' | '256' | 'none';

/**
 * What art the terminal can show. PADDOCK_TUI_ART (truecolor, 256, off)
 * decides first, then NO_COLOR and FORCE_COLOR, then COLORTERM and TERM.
 * Sixteen colours are too few for the art: none.
 */
export function detectArt(env: NodeJS.ProcessEnv, isTTY: boolean, plain = false): ArtMode {
  if (plain) return 'none';
  const pick = env.PADDOCK_TUI_ART?.toLowerCase();
  if (pick === 'off' || pick === '0' || pick === 'none') return 'none';
  if (env.NO_COLOR !== undefined && env.NO_COLOR !== '') return 'none';
  if (pick === 'truecolor' || pick === '24bit') return 'truecolor';
  if (pick === '256') return '256';
  const force = env.FORCE_COLOR;
  if (force !== undefined) {
    if (force === '0' || force === 'false') return 'none';
    if (force === '3') return 'truecolor';
    if (force === '2') return '256';
    // FORCE_COLOR=1 or true: the 16 colours, so no art.
    return 'none';
  }
  if (!isTTY) return 'none';
  const term = (env.TERM ?? '').toLowerCase();
  if (term === 'dumb' || term === '') return 'none';
  const colorterm = (env.COLORTERM ?? '').toLowerCase();
  if (colorterm === 'truecolor' || colorterm === '24bit') return 'truecolor';
  if (/(^|-)direct$|kitty|alacritty|foot|ghostty|wezterm|contour/.test(term)) return 'truecolor';
  if (['iTerm.app', 'WezTerm', 'vscode', 'ghostty'].includes(env.TERM_PROGRAM ?? '')) return 'truecolor';
  if (term.includes('256')) return '256';
  return 'none';
}

export interface Cell {
  ch: ' ' | '▀' | '▄';
  fg: Rgb | null;
  bg: Rgb | null;
}

/** The cells of one frame: h/2 rows of w cells (an odd last row has no bottom pixel). */
export function halfBlock(s: Sprite, frame: number): Cell[][] {
  const f = s.frames[((frame % s.frames.length) + s.frames.length) % s.frames.length] as Pixel[];
  const rows: Cell[][] = [];
  for (let y = 0; y < s.h; y += 2) {
    const row: Cell[] = [];
    for (let x = 0; x < s.w; x++) {
      const top = f[y * s.w + x] ?? null;
      const bottom = y + 1 < s.h ? (f[(y + 1) * s.w + x] ?? null) : null;
      if (top && bottom) row.push({ ch: '▀', fg: top, bg: bottom });
      else if (top) row.push({ ch: '▀', fg: top, bg: null });
      else if (bottom) row.push({ ch: '▄', fg: bottom, bg: null });
      else row.push({ ch: ' ', fg: null, bg: null });
    }
    rows.push(row);
  }
  return rows;
}

const CUBE = [0, 95, 135, 175, 215, 255];

function nearestCube(v: number): number {
  let best = 0;
  for (let i = 1; i < CUBE.length; i++) if (Math.abs((CUBE[i] as number) - v) < Math.abs((CUBE[best] as number) - v)) best = i;
  return best;
}

/** The nearest of the 256 colours: the 6x6x6 cube (16-231) or the gray ramp (232-255). */
export function rgbTo256(c: Rgb): number {
  const [r, g, b] = c;
  const ri = nearestCube(r);
  const gi = nearestCube(g);
  const bi = nearestCube(b);
  const cube = 16 + 36 * ri + 6 * gi + bi;
  const cr = CUBE[ri] as number;
  const cg = CUBE[gi] as number;
  const cb = CUBE[bi] as number;
  const avg = (r + g + b) / 3;
  const gi2 = Math.max(0, Math.min(23, Math.round((avg - 8) / 10)));
  const gv = 8 + gi2 * 10;
  const dCube = (r - cr) ** 2 + (g - cg) ** 2 + (b - cb) ** 2;
  const dGray = (r - gv) ** 2 + (g - gv) ** 2 + (b - gv) ** 2;
  return dGray < dCube ? 232 + gi2 : cube;
}

function sgrColor(c: Rgb | null, mode: ArtMode, bg: boolean): string {
  if (!c) return bg ? '49' : '39';
  if (mode === 'truecolor') return `${bg ? 48 : 38};2;${c[0]};${c[1]};${c[2]}`;
  return `${bg ? 48 : 38};5;${rgbTo256(c)}`;
}

function key(c: Rgb | null, mode: ArtMode): string {
  return c ? sgrColor(c, mode, false) : '';
}

/**
 * One row of cells as text with SGR codes: a code only where the colour
 * changes, and a reset at the end.
 */
export function rowText(cells: Cell[], mode: ArtMode): string {
  if (mode === 'none') return ' '.repeat(cells.length);
  let out = '';
  let fg = '';
  let bg = '';
  for (const c of cells) {
    const nf = key(c.fg, mode);
    const nb = key(c.bg, mode);
    const codes: string[] = [];
    // A blank cell shows only its background: its foreground can stay.
    if (c.ch !== ' ' && nf !== fg) {
      codes.push(sgrColor(c.fg, mode, false));
      fg = nf;
    }
    if (nb !== bg) {
      codes.push(sgrColor(c.bg, mode, true));
      bg = nb;
    }
    if (codes.length > 0) out += `\u001b[${codes.join(';')}m`;
    out += c.ch;
  }
  return out + (fg || bg ? '\u001b[39;49m' : '');
}

/** One row of cells as the painter keeps it: the same styles as the parsed text of rowText. */
export function screenRow(cells: Cell[], mode: ArtMode): Row {
  const row: Row = { ch: [], st: [] };
  for (const c of cells) {
    row.ch.push(c.ch);
    row.st.push(mode === 'none' ? '' : cellStyle(c.ch, c.fg ? sgrColor(c.fg, mode, false) : '', c.bg ? sgrColor(c.bg, mode, true) : ''));
  }
  return row;
}

export interface CellChange {
  x: number;
  y: number;
  cell: Cell;
}

function sameColor(a: Rgb | null, b: Rgb | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

export function sameCell(a: Cell, b: Cell): boolean {
  return a.ch === b.ch && sameColor(a.fg, b.fg) && sameColor(a.bg, b.bg);
}

/** The cells of `next` that differ from `prev` (all of them when prev is null). */
export function diffCells(prev: Cell[][] | null, next: Cell[][]): CellChange[] {
  const out: CellChange[] = [];
  next.forEach((row, y) =>
    row.forEach((cell, x) => {
      const old = prev?.[y]?.[x];
      if (!old || !sameCell(old, cell)) out.push({ x, y, cell });
    }),
  );
  return out;
}

/** A run of changed cells in one row: where it starts in the sprite, and its text with SGR codes. */
export interface Run {
  x: number;
  y: number;
  text: string;
}

/** The changed cells as runs of neighbours in a row, each as text. */
export function changeRuns(changes: CellChange[], mode: ArtMode): Run[] {
  if (mode === 'none') return [];
  const sorted = [...changes].sort((a, b) => a.y - b.y || a.x - b.x);
  const runs: Run[] = [];
  let i = 0;
  while (i < sorted.length) {
    const first = sorted[i] as CellChange;
    const run: Cell[] = [first.cell];
    let j = i + 1;
    while (j < sorted.length && (sorted[j] as CellChange).y === first.y && (sorted[j] as CellChange).x === first.x + run.length) {
      run.push((sorted[j] as CellChange).cell);
      j++;
    }
    runs.push({ x: first.x, y: first.y, text: rowText(run, mode) });
    i = j;
  }
  return runs;
}

/**
 * The bytes that draw runs at a place on the screen (0-based column and
 * row of the sprite's top left cell): the cursor jumps to each run. The
 * cursor and its attributes are saved before and restored after (DECSC,
 * DECRC), so the interface's own drawing goes on where it was.
 */
export function drawRuns(runs: readonly Run[], at: { x: number; y: number }): string {
  if (runs.length === 0) return '';
  let out = '\u001b7';
  for (const r of runs) out += `\u001b[${at.y + r.y + 1};${at.x + r.x + 1}H` + r.text;
  return out + '\u001b[0m\u001b8';
}

/** The bytes that draw the changed cells at a place on the screen (see drawRuns). */
export function drawChanges(changes: CellChange[], at: { x: number; y: number }, mode: ArtMode): string {
  return drawRuns(changeRuns(changes, mode), at);
}

/**
 * A sprite made ready for one art mode: the cells and the row texts of
 * every frame, and the runs of each step from a frame to the next. It is
 * made once, when the sprite first shows; an animation step only puts
 * the cursor moves in front of the ready texts.
 */
export interface PreparedSprite {
  cells: Cell[][][];
  rows: string[][];
  /** The cells of every frame as the painter keeps them (screen.ts). */
  screen: Row[][];
  /** The runs that turn frame `from` into frame `to`. */
  step(from: number, to: number): readonly Run[];
}

export function prepareSprite(s: Sprite, mode: ArtMode): PreparedSprite {
  const n = s.frames.length;
  const cells = s.frames.map((_, f) => halfBlock(s, f));
  const rows = cells.map((frame) => frame.map((r) => rowText(r, mode)));
  const steps = new Map<number, readonly Run[]>();
  const make = (from: number, to: number): readonly Run[] => changeRuns(diffCells(cells[from] as Cell[][], cells[to] as Cell[][]), mode);
  // The loop goes frame by frame: those steps are made now, others when asked.
  if (n > 1) for (let f = 0; f < n; f++) steps.set(f * n + ((f + 1) % n), make(f, (f + 1) % n));
  const screen = cells.map((frame) => frame.map((r) => screenRow(r, mode)));
  return {
    cells,
    rows,
    screen,
    step(from, to) {
      const k = from * n + to;
      let r = steps.get(k);
      if (!r) {
        r = make(from, to);
        steps.set(k, r);
      }
      return r;
    },
  };
}
