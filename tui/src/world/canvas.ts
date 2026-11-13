// The canvas of the world: a grid of terminal cells. Each cell holds two
// pixels (top and bottom, as the sprites) and can hold one character of
// text over them. With art, the pixels show as half blocks and the text
// lies on the pixel under it. Without art (NO_COLOR, 16 colours, --plain)
// the pixels do not exist: the same scene draws itself with characters
// only, and bold, dim and inverse carry what the colours carried.

import { rgbTo256, type ArtMode } from '../art.js';
import { charCells, cleanText } from '../clean.js';
import type { Row } from '../screen.js';
import type { Pixel, Rgb } from '../sprites.js';

/** One of the 16 colours of the terminal, as its SGR code (30-37, 90-97). */
export type Ansi = number;

export interface TextStyle {
  /** A colour of the art, or one of the terminal's 16. None: the terminal's own. */
  fg?: Rgb | Ansi;
  /** The background. None: the pixel under the text, or the terminal's own. */
  bg?: Rgb;
  bold?: boolean;
  dim?: boolean;
  inverse?: boolean;
  underline?: boolean;
}

interface TextCell {
  ch: string;
  style: TextStyle;
}

/** A picture in pixels: w x h, row by row, null is transparent. */
export interface Bitmap {
  w: number;
  h: number;
  px: readonly Pixel[];
}

export interface CanvasOptions {
  mode: ArtMode;
  /** --plain: no attribute at all, only characters. */
  plain?: boolean;
  /** A light terminal: text on the terminal's own background takes the darker shades. */
  light?: boolean;
  /**
   * How large a pixel of the art is: 1, or 2 for a picture at twice its
   * size on a wide screen. The places of pixels count pixels of the art;
   * the places of text always count cells.
   */
  scale?: number;
}

/** What a pixel is: the ground that words may lie on (grass, water, a path), or a thing of the art. */
export type PixelKind = 'ground' | 'art';

export class Canvas {
  readonly w: number;
  readonly h: number;
  readonly mode: ArtMode;
  /** Whether pixels show. Without them, a scene draws with characters. */
  readonly art: boolean;
  readonly light: boolean;
  /** The size of a pixel of the art, in pixels of the screen (half cells). */
  readonly scale: number;
  /** The size of the picture in pixels of the art. */
  readonly pw: number;
  readonly ph: number;
  private plain: boolean;
  private px: Pixel[];
  /** For each pixel of the screen: 1 where a thing of the art is, 0 on the ground. */
  private solid: Uint8Array;
  private kind = 1;
  private tx: (TextCell | null)[];
  /** The rectangle of pixels that drawing reaches, or null: all of the canvas. */
  private clip: { x0: number; y0: number; x1: number; y1: number } | null = null;

  constructor(w: number, h: number, o: CanvasOptions) {
    this.w = Math.max(0, w);
    this.h = Math.max(0, h);
    this.mode = o.mode;
    this.art = o.mode !== 'none';
    this.plain = o.plain === true;
    this.light = o.light === true;
    this.scale = Math.max(1, Math.floor(o.scale ?? 1));
    this.pw = Math.floor(this.w / this.scale);
    this.ph = Math.floor((this.h * 2) / this.scale);
    this.solid = new Uint8Array(this.w * this.h * 2);
    this.px = new Array<Pixel>(this.w * this.h * 2).fill(null);
    this.tx = new Array<TextCell | null>(this.w * this.h).fill(null);
  }

  /** Keep the pixels that are drawn from now on inside a rectangle (the walls of a room), or free them again. */
  setClip(r: { x: number; y: number; w: number; h: number } | null): void {
    this.clip = r ? { x0: r.x, y0: r.y, x1: r.x + r.w, y1: r.y + r.h } : null;
  }

  /** What the pixels that are drawn from now on are: things of the art, or the ground under them. */
  setKind(k: PixelKind): void {
    this.kind = k === 'art' ? 1 : 0;
  }

  private inside(x: number, y: number): boolean {
    if (x < 0 || y < 0 || x >= this.pw || y >= this.ph) return false;
    const k = this.clip;
    return k === null || (x >= k.x0 && x < k.x1 && y >= k.y0 && y < k.y1);
  }

  /** One pixel of the art on the screen: a square of `scale` pixels. */
  private put(x: number, y: number, c: Pixel): void {
    const s = this.scale;
    if (s === 1) {
      const i = y * this.w + x;
      this.px[i] = c;
      this.solid[i] = this.kind;
      return;
    }
    for (let dy = 0; dy < s; dy++) {
      const row = (y * s + dy) * this.w + x * s;
      for (let dx = 0; dx < s; dx++) {
        this.px[row + dx] = c;
        this.solid[row + dx] = this.kind;
      }
    }
  }

  /** One pixel. `y` counts pixels: two per row of cells (at scale 1). */
  pixel(x: number, y: number, c: Pixel): void {
    if (!this.art || !this.inside(x, y)) return;
    this.put(x, y, c);
  }

  pixelAt(x: number, y: number): Pixel {
    if (x < 0 || y < 0 || x >= this.pw || y >= this.ph) return null;
    return this.px[y * this.scale * this.w + x * this.scale] ?? null;
  }

  /** A rectangle of pixels. */
  fill(x: number, y: number, w: number, h: number, c: Pixel): void {
    if (!this.art) return;
    for (let yy = Math.max(0, y); yy < Math.min(this.ph, y + h); yy++) {
      for (let xx = Math.max(0, x); xx < Math.min(this.pw, x + w); xx++) if (this.inside(xx, yy)) this.put(xx, yy, c);
    }
  }

  /** The outline of a rectangle of pixels, one pixel thick. */
  outline(x: number, y: number, w: number, h: number, c: Pixel): void {
    this.fill(x, y, w, 1, c);
    this.fill(x, y + h - 1, w, 1, c);
    this.fill(x, y, 1, h, c);
    this.fill(x + w - 1, y, 1, h, c);
  }

  /** A picture at a place in pixels. Transparent pixels leave what is there. `map` changes each colour. */
  blit(b: Bitmap, x: number, y: number, map?: (c: Rgb) => Rgb): void {
    if (!this.art) return;
    for (let sy = 0; sy < b.h; sy++) {
      const yy = y + sy;
      if (yy < 0 || yy >= this.ph) continue;
      for (let sx = 0; sx < b.w; sx++) {
        const xx = x + sx;
        if (!this.inside(xx, yy)) continue;
        const p = b.px[sy * b.w + sx];
        if (p) this.put(xx, yy, map ? map(p) : p);
      }
    }
  }

  /** Change every pixel that is set in a rectangle (a tint, a fade). */
  recolor(x: number, y: number, w: number, h: number, map: (c: Rgb) => Rgb): void {
    if (!this.art) return;
    const kind = this.kind;
    for (let yy = Math.max(0, y); yy < Math.min(this.ph, y + h); yy++) {
      for (let xx = Math.max(0, x); xx < Math.min(this.pw, x + w); xx++) {
        const i = yy * this.scale * this.w + xx * this.scale;
        const p = this.px[i];
        if (!p) continue;
        // The pixel stays what it was: ground or a thing.
        this.kind = this.solid[i] as number;
        this.put(xx, yy, map(p));
      }
    }
    this.kind = kind;
  }

  /** The cells in which words lie on a thing of the art. A picture whose words stand on signs and on the ground has none. */
  clashes(): { x: number; y: number }[] {
    const out: { x: number; y: number }[] = [];
    if (!this.art) return out;
    for (let y = 0; y < this.h; y++) {
      for (let x = 0; x < this.w; x++) {
        if (!this.tx[y * this.w + x]) continue;
        const top = y * 2 * this.w + x;
        const bottom = top + this.w;
        if ((this.px[top] && this.solid[top]) || (this.px[bottom] && this.solid[bottom])) out.push({ x, y });
      }
    }
    return out;
  }

  /**
   * Text at a cell, cut at the right edge. `y` counts rows of cells. A
   * character that takes two cells keeps both. Returns the cells used.
   */
  text(x: number, y: number, s: string, style: TextStyle = {}): number {
    if (y < 0 || y >= this.h) return 0;
    let at = x;
    // Text of a run can come from a sandbox: no escape or invisible character reaches a cell.
    for (const ch of cleanText(s)) {
      const n = charCells(ch);
      if (n === 0) continue;
      if (at + n > this.w) break;
      if (at >= 0) {
        this.tx[y * this.w + at] = { ch, style };
        if (n === 2) this.tx[y * this.w + at + 1] = { ch: '', style };
      }
      at += n;
    }
    return at - x;
  }

  /** Take the text of a rectangle of cells away: the pixels under it show again. */
  clearText(x: number, y: number, w: number, h: number): void {
    for (let yy = Math.max(0, y); yy < Math.min(this.h, y + h); yy++) {
      for (let xx = Math.max(0, x); xx < Math.min(this.w, x + w); xx++) this.tx[yy * this.w + xx] = null;
    }
  }

  private color(c: Rgb | Ansi, bg: boolean): string {
    if (typeof c === 'number') return String(bg ? c + 10 : c);
    if (this.mode === 'truecolor') return `${bg ? 48 : 38};2;${c[0]};${c[1]};${c[2]}`;
    return `${bg ? 48 : 38};5;${rgbTo256(c)}`;
  }

  /** The style of a cell of text, in the fixed order of the painter (screen.ts). */
  private textStyle(t: TextCell, under: Pixel): string {
    if (this.plain) return '';
    const s = t.style;
    const marks = (s.underline ? ';4' : '') + (s.inverse ? ';7' : '');
    const bgc = this.art ? (s.bg ?? under) : null;
    const bg = bgc ? ';' + this.color(bgc, true) : '';
    // A blank cell shows only its background, and an underline. An inverse blank shows the colour of the text.
    if (t.ch === ' ' && !s.inverse) return (marks + bg).slice(1);
    let out = (s.bold ? ';1' : '') + (s.dim ? ';2' : '') + marks;
    // Without art there is no colour at all: the attributes carry the roles.
    if (this.art && s.fg !== undefined) out += ';' + this.color(s.fg, false);
    return (out + bg).slice(1);
  }

  /** The cells as the painter keeps them. */
  rows(): Row[] {
    const out: Row[] = [];
    const cache = this.art ? cellCache(this.mode) : null;
    for (let y = 0; y < this.h; y++) {
      const row: Row = { ch: new Array<string>(this.w), st: new Array<string>(this.w) };
      for (let x = 0; x < this.w; x++) {
        const top = this.art ? (this.px[y * 2 * this.w + x] ?? null) : null;
        const bottom = this.art ? (this.px[(y * 2 + 1) * this.w + x] ?? null) : null;
        const t = this.tx[y * this.w + x];
        if (t) {
          row.ch[x] = t.ch;
          row.st[x] = this.textStyle(t, top ?? bottom);
        } else if (top || bottom) {
          // The cell of two pixels: made once for each pair of colours, then kept.
          const key = (top ? code(top) + 1 : 0) * PAIR + (bottom ? code(bottom) + 1 : 0);
          let cell = (cache as Map<number, Cell>).get(key);
          if (!cell) {
            cell = this.cell(top, bottom);
            if ((cache as Map<number, Cell>).size > CELLS_KEPT) (cache as Map<number, Cell>).clear();
            (cache as Map<number, Cell>).set(key, cell);
          }
          row.ch[x] = cell.ch;
          row.st[x] = cell.st;
        } else {
          row.ch[x] = ' ';
          row.st[x] = '';
        }
      }
      out.push(row);
    }
    return out;
  }

  /** The half block of two pixels. */
  private cell(top: Pixel, bottom: Pixel): Cell {
    if (top && bottom) return sameRgb(top, bottom) ? { ch: ' ', st: this.color(top, true) } : { ch: '▀', st: `${this.color(top, false)};${this.color(bottom, true)}` };
    if (top) return { ch: '▀', st: this.color(top, false) };
    return { ch: '▄', st: this.color(bottom as Rgb, false) };
  }

  /** The characters of each row, without styles and without the blanks at the end (tests, plain text). */
  lines(): string[] {
    return this.rows().map((r) => r.ch.join('').trimEnd());
  }
}

interface Cell {
  ch: string;
  st: string;
}

/** A colour as one number, and the room for a pair of them in one key. */
const code = (c: Rgb): number => (c[0] << 16) | (c[1] << 8) | c[2];
const PAIR = 1 << 25;
/** A fade makes new colours at each frame: the cells of old ones go. */
const CELLS_KEPT = 20_000;
const cellCaches = new Map<ArtMode, Map<number, Cell>>();

/** The cells of the pairs of colours that were drawn, for a colour mode. */
function cellCache(mode: ArtMode): Map<number, Cell> {
  let c = cellCaches.get(mode);
  if (!c) {
    c = new Map();
    cellCaches.set(mode, c);
  }
  return c;
}

function sameRgb(a: Rgb, b: Rgb): boolean {
  return a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

/** One row of cells as text with SGR codes, for Ink: a code only where the style changes. */
export function rowText(row: Row): string {
  let out = '';
  let cur = '';
  for (let x = 0; x < row.ch.length; x++) {
    const ch = row.ch[x] as string;
    if (ch === '') continue;
    const st = row.st[x] as string;
    if (st !== cur) {
      out += st ? `\u001b[0;${st}m` : '\u001b[0m';
      cur = st;
    }
    out += ch;
  }
  return cur ? out + '\u001b[0m' : out;
}

/** Mix two colours: t = 0 gives a, t = 1 gives b. */
export function mix(a: Rgb, b: Rgb, t: number): Rgb {
  const k = Math.max(0, Math.min(1, t));
  return [Math.round(a[0] + (b[0] - a[0]) * k), Math.round(a[1] + (b[1] - a[1]) * k), Math.round(a[2] + (b[2] - a[2]) * k)];
}

/** A picture from rows of letters: each letter is a colour of the palette, '.' is transparent. */
export function bitmap(rows: readonly string[], palette: Record<string, Rgb>): Bitmap {
  const w = Math.max(0, ...rows.map((r) => r.length));
  const px: Pixel[] = [];
  for (const r of rows) {
    for (let x = 0; x < w; x++) {
      const ch = r[x] ?? '.';
      px.push(ch === '.' || ch === ' ' ? null : (palette[ch] ?? null));
    }
  }
  return { w, h: rows.length, px };
}

/**
 * A picture at 1/n of its size. Each new pixel takes the colour that most
 * of its n x n old pixels have; the ink of the outline wins a tie, so a
 * small figure keeps its line. Fewer than a third set: transparent.
 */
export function shrink(b: Bitmap, n: number, ink?: Rgb): Bitmap {
  const w = Math.floor(b.w / n);
  const h = Math.floor(b.h / n);
  const px: Pixel[] = [];
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const count = new Map<string, { c: Rgb; n: number }>();
      let set = 0;
      for (let dy = 0; dy < n; dy++) {
        for (let dx = 0; dx < n; dx++) {
          const p = b.px[(y * n + dy) * b.w + x * n + dx];
          if (!p) continue;
          set++;
          const k = `${p[0]},${p[1]},${p[2]}`;
          const e = count.get(k);
          if (e) e.n++;
          else count.set(k, { c: p, n: 1 });
        }
      }
      if (set * 3 < n * n) {
        px.push(null);
        continue;
      }
      let best: { c: Rgb; n: number } | null = null;
      for (const e of count.values()) {
        const isInk = ink !== undefined && sameRgb(e.c, ink);
        // The ink counts a little less than a flat colour: a small face stays a face, not a blot.
        const score = isInk ? e.n * 0.75 : e.n;
        if (!best || score > (ink !== undefined && sameRgb(best.c, ink) ? best.n * 0.75 : best.n)) best = e;
      }
      px.push(best ? best.c : null);
    }
  }
  return { w, h, px };
}
