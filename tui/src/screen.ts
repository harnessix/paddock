// The painter: it keeps the cells that the terminal shows, and writes only
// the cells that differ in a new frame. Ink renders each frame as text;
// the painter turns it into cells, compares and writes the changed runs.
// Nothing is erased and no line is written again when it did not change,
// so the screen cannot flicker. The art layer draws its sprites through
// the same cells, so both always agree on what the terminal shows.

import { charCells } from './clean.js';

/** One row of the screen: a character and a style per cell. */
export interface Row {
  /** The character of each cell. '' is the second cell of a wide character. */
  ch: string[];
  /** The style of each cell: SGR codes in a fixed order, '' is the default. */
  st: string[];
}

/** A rectangle of cells that lies over the frame of Ink: a sprite. */
export interface Overlay {
  x: number;
  y: number;
  rows: readonly Row[];
}

export const SYNC_ON = '\u001b[?2026h';
export const SYNC_OFF = '\u001b[?2026l';
const RESET = '\u001b[0m';

interface Sgr {
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  inverse: boolean;
  strike: boolean;
  fg: string;
  bg: string;
}

function plainSgr(): Sgr {
  return { bold: false, dim: false, italic: false, underline: false, inverse: false, strike: false, fg: '', bg: '' };
}

/** The style of a cell as SGR codes in a fixed order. */
function styleOf(s: Sgr): string {
  let out = '';
  if (s.bold) out += ';1';
  if (s.dim) out += ';2';
  if (s.italic) out += ';3';
  if (s.underline) out += ';4';
  if (s.inverse) out += ';7';
  if (s.strike) out += ';9';
  if (s.fg) out += ';' + s.fg;
  if (s.bg) out += ';' + s.bg;
  return out.slice(1);
}

/** The style of a blank cell: only what a space shows. */
function blankStyleOf(s: Sgr): string {
  if (s.underline || s.inverse || s.strike) return styleOf(s);
  return s.bg;
}

/**
 * The style of a cell from a foreground and a background code (for
 * example "38;5;196" and "48;5;46", or ''). A blank cell keeps only its
 * background, as in a parsed frame.
 */
export function cellStyle(ch: string, fg: string, bg: string): string {
  if (ch === ' ') return bg;
  return fg && bg ? `${fg};${bg}` : fg || bg;
}

function applySgr(s: Sgr, params: string): void {
  const p = params === '' ? [0] : params.split(';').map((x) => (x === '' ? 0 : Number(x)));
  for (let i = 0; i < p.length; i++) {
    const c = p[i] as number;
    if (c === 0) Object.assign(s, plainSgr());
    else if (c === 1) s.bold = true;
    else if (c === 2) s.dim = true;
    else if (c === 3) s.italic = true;
    else if (c === 4) s.underline = true;
    else if (c === 7) s.inverse = true;
    else if (c === 9) s.strike = true;
    else if (c === 22) s.bold = s.dim = false;
    else if (c === 23) s.italic = false;
    else if (c === 24) s.underline = false;
    else if (c === 27) s.inverse = false;
    else if (c === 29) s.strike = false;
    else if ((c >= 30 && c <= 37) || (c >= 90 && c <= 97)) s.fg = String(c);
    else if (c === 39) s.fg = '';
    else if ((c >= 40 && c <= 47) || (c >= 100 && c <= 107)) s.bg = String(c);
    else if (c === 49) s.bg = '';
    else if (c === 38 || c === 48) {
      const kind = p[i + 1];
      let code = '';
      if (kind === 5 && i + 2 < p.length) {
        code = `${c};5;${p[i + 2]}`;
        i += 2;
      } else if (kind === 2 && i + 4 < p.length) {
        code = `${c};2;${p[i + 2]};${p[i + 3]};${p[i + 4]}`;
        i += 4;
      } else {
        // A colour code that is cut short: nothing more to read.
        i = p.length;
      }
      if (code) {
        if (c === 38) s.fg = code;
        else s.bg = code;
      }
    }
  }
}

export function blankRow(columns: number): Row {
  return { ch: new Array<string>(columns).fill(' '), st: new Array<string>(columns).fill('') };
}

// An SGR sequence, any other CSI sequence, an OSC string (a link), or one character.
const TOKEN = /\u001b\[([\d;]*)m|\u001b\[[\d;?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)|[\s\S]/gu;

/** One line of a frame (text with SGR codes) as `columns` cells. */
export function parseLine(line: string, columns: number): Row {
  const row = blankRow(columns);
  const s = plainSgr();
  let style = '';
  let blank = '';
  let x = 0;
  TOKEN.lastIndex = 0;
  for (let m = TOKEN.exec(line); m !== null; m = TOKEN.exec(line)) {
    const tok = m[0];
    if (tok.length > 1 && tok.charCodeAt(0) === 0x1b) {
      if (m[1] !== undefined) {
        applySgr(s, m[1]);
        style = styleOf(s);
        blank = blankStyleOf(s);
      }
      continue;
    }
    const w = charCells(tok);
    if (w === 0) {
      // A combining mark belongs to the cell before it.
      if (x > 0 && tok.charCodeAt(0) >= 0x20) {
        const at = row.ch[x - 1] === '' && x > 1 ? x - 2 : x - 1;
        row.ch[at] += tok;
      }
      continue;
    }
    if (x + w > columns) break;
    row.ch[x] = tok;
    row.st[x] = tok === ' ' ? blank : style;
    if (w === 2) {
      row.ch[x + 1] = '';
      row.st[x + 1] = style;
    }
    x += w;
  }
  return row;
}

interface StyleInfo {
  /** The codes of bold, dim, italic, underline, inverse and strike. */
  attrs: string;
  fg: string;
  bg: string;
}

const infos = new Map<string, StyleInfo>();

/** The parts of a style. Kept per style: a screen has few of them. */
function infoOf(st: string): StyleInfo {
  let v = infos.get(st);
  if (v) return v;
  v = { attrs: '', fg: '', bg: '' };
  const p = st === '' ? [] : st.split(';');
  for (let i = 0; i < p.length; i++) {
    const c = p[i] as string;
    if (c === '38' || c === '48') {
      const n = p[i + 1] === '2' ? 5 : 3;
      const code = p.slice(i, i + n).join(';');
      if (c === '38') v.fg = code;
      else v.bg = code;
      i += n - 1;
    } else if (c.length === 1) v.attrs += (v.attrs ? ';' : '') + c;
    else if (c.startsWith('3') || c.startsWith('9')) v.fg = c;
    else v.bg = c;
  }
  if (infos.size > 4096) infos.clear();
  infos.set(st, v);
  return v;
}

/** The style of a blank cell in place of a cell with the style `st`: only what a space shows. */
export function blankStyle(st: string): string {
  const i = infoOf(st);
  return /[479]/.test(i.attrs) ? st : i.bg;
}

/** The style that the terminal has after the last cell written, or null: not known. */
interface Pen {
  cur: StyleInfo | null;
}

/** The codes that change the pen to the style of a cell: only what differs. */
function penTo(pen: Pen, ch: string, st: string): string {
  const to = infoOf(st);
  const cur = pen.cur;
  if (cur === null || cur.attrs !== to.attrs) {
    pen.cur = to;
    return st ? `\u001b[0;${st}m` : RESET;
  }
  // A blank cell without attributes shows only its background: the foreground can stay.
  const fg = ch === ' ' && to.attrs === '' ? cur.fg : to.fg;
  if (fg === cur.fg && to.bg === cur.bg) return '';
  let codes = '';
  if (fg !== cur.fg) codes = fg || '39';
  if (to.bg !== cur.bg) codes += (codes ? ';' : '') + (to.bg || '49');
  pen.cur = fg === to.fg ? to : { attrs: to.attrs, fg, bg: to.bg };
  return `\u001b[${codes}m`;
}

/** The runs of cells to write in a row: the 0-based column where each starts, its text with SGR codes, and its cells. */
function runsOf(shown: Row, want: Row, columns: number, pen: Pen): { x: number; text: string; n: number }[] {
  const changed: boolean[] = new Array<boolean>(columns).fill(false);
  let any = false;
  for (let x = 0; x < columns; x++) {
    if (shown.ch[x] !== want.ch[x] || shown.st[x] !== want.st[x]) {
      changed[x] = true;
      any = true;
    }
  }
  if (!any) return [];
  // A wide character is written whole, and so is one that a write cuts in two.
  for (let x = 0; x < columns; x++) {
    if (!changed[x]) continue;
    if ((want.ch[x] === '' || shown.ch[x] === '') && x > 0) changed[x - 1] = true;
    if (x + 1 < columns && (want.ch[x + 1] === '' || shown.ch[x + 1] === '')) changed[x + 1] = true;
  }
  const out: { x: number; text: string; n: number }[] = [];
  let x = 0;
  while (x < columns) {
    if (!changed[x]) {
      x++;
      continue;
    }
    let start = x;
    if (want.ch[start] === '' && start > 0) start--;
    let end = x;
    // Cells that stay the same between two changes cost less than a cursor move.
    for (let j = x + 1; j < columns && j - end - 1 <= GAP; j++) if (changed[j]) end = j;
    if (end + 1 < columns && want.ch[end + 1] === '') end++;
    let text = '';
    for (let j = start; j <= end; j++) {
      const ch = want.ch[j] as string;
      if (ch === '') continue;
      text += penTo(pen, ch, want.st[j] as string) + ch;
    }
    out.push({ x: start, text, n: end - start + 1 });
    x = end + 1;
  }
  return out;
}

const GAP = 3;

/** Put the cells of `src` into a row at column `x`. A wide character that the edge would cut becomes a blank. */
function put(row: Row, x: number, src: Row, columns: number): void {
  const end = x + src.ch.length;
  if (x > 0 && x < columns && row.ch[x] === '') {
    row.ch[x - 1] = ' ';
    row.st[x - 1] = blankStyle(row.st[x - 1] as string);
  }
  if (end > 0 && end < columns && row.ch[end] === '') {
    row.ch[end] = ' ';
    row.st[end] = blankStyle(row.st[end] as string);
  }
  for (let dx = 0; dx < src.ch.length; dx++) {
    const at = x + dx;
    if (at < 0 || at >= columns) continue;
    row.ch[at] = src.ch[dx] as string;
    row.st[at] = src.st[dx] as string;
  }
}

export interface PainterOptions {
  /** Writes to the terminal. Returns false when the terminal is not ready for more. */
  write: (s: string) => boolean;
  /** The size of the terminal. */
  size: () => { columns: number; rows: number };
  /** The sprites that lie over the frame, where they stand now. */
  overlays?: () => Overlay[];
  /** The shortest time between two frames, in ms. */
  minFrameMs?: number;
  now?: () => number;
}

/** Counters for tests and for `PADDOCK_TUI_STATS`. */
export interface PainterStats {
  /** Frames that Ink handed over. */
  frames: number;
  /** Frames that changed at least one cell. */
  painted: number;
  /** Cells written, in frames and in patches. */
  cells: number;
  bytes: number;
  /** Times the whole screen was cleared: the first frame and each resize. */
  clears: number;
}

export class Painter {
  private o: PainterOptions;
  private shown: Row[] = [];
  private columns = 0;
  private rows = 0;
  private lastText: string | null = null;
  private lastLines: string[] = [];
  private parsed: Row[] = [];
  private pending: string | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private lastFlush = -Infinity;
  private waiting = false;
  private stopped = false;
  readonly stats: PainterStats = { frames: 0, painted: 0, cells: 0, bytes: 0, clears: 0 };

  constructor(o: PainterOptions) {
    this.o = o;
  }

  private now(): number {
    return this.o.now ? this.o.now() : Date.now();
  }

  /** A frame of Ink. Frames that come faster than the terminal should get them are joined. */
  frame(text: string): void {
    if (this.stopped) return;
    this.stats.frames++;
    this.pending = text;
    if (this.timer || this.waiting) return;
    const wait = (this.o.minFrameMs ?? 8) - (this.now() - this.lastFlush);
    if (wait <= 0) this.flush();
    else {
      this.timer = setTimeout(() => {
        this.timer = null;
        this.flush();
      }, wait);
    }
  }

  /** The terminal took what it got: the next frame can go. */
  drained(): void {
    if (!this.waiting) return;
    this.waiting = false;
    if (this.pending !== null) this.flush();
  }

  /** Forget what the terminal shows: the next frame draws every cell (after a resize, or when another program drew). */
  invalidate(): void {
    this.columns = 0;
    if (this.pending === null) this.pending = this.lastText;
    if (this.pending !== null && !this.timer && !this.waiting) this.flush();
  }

  /** Write the frame that waits, now. */
  flush(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    const text = this.pending;
    this.pending = null;
    if (text === null || this.stopped) return;
    this.lastFlush = this.now();
    const { columns, rows } = this.o.size();
    let out = '';
    if (columns !== this.columns || rows !== this.rows) {
      // A new size: the terminal moved or cut what it showed.
      this.columns = columns;
      this.rows = rows;
      this.shown = Array.from({ length: rows }, () => blankRow(columns));
      this.lastLines = [];
      this.parsed = [];
      this.stats.clears++;
      out += `${RESET}\u001b[H\u001b[2J`;
    }
    const lines = text === this.lastText && this.lastLines.length > 0 ? this.lastLines : text.split('\n');
    const want: Row[] = [];
    for (let y = 0; y < rows; y++) {
      const line = lines[y] ?? '';
      // A line with the same text as in the last frame has the same cells.
      const old = this.parsed[y];
      want.push(old !== undefined && this.lastLines[y] === line ? old : parseLine(line, columns));
    }
    this.lastText = text;
    this.lastLines = lines;
    this.parsed = want;
    const over = this.o.overlays?.() ?? [];
    const target = over.length > 0 ? want.slice() : want;
    for (const o of over) this.stamp(target, want, o);
    let cells = 0;
    const pen: Pen = { cur: null };
    for (let y = 0; y < rows; y++) {
      const t = target[y] as Row;
      const s = this.shown[y] as Row;
      if (t === s) continue;
      for (const r of runsOf(s, t, columns, pen)) {
        out += `\u001b[${y + 1};${r.x + 1}H${r.text}`;
        cells += r.n;
      }
      this.shown[y] = t;
    }
    if (out === '') return;
    this.stats.painted++;
    this.stats.cells += cells;
    this.send(SYNC_ON + out + RESET + SYNC_OFF);
  }

  /** Put the rows of an overlay into the rows of a frame. A row is copied before its first change. */
  private stamp(target: Row[], base: Row[], o: Overlay): void {
    for (let dy = 0; dy < o.rows.length; dy++) {
      const y = o.y + dy;
      if (y < 0 || y >= this.rows) continue;
      const src = o.rows[dy] as Row;
      let row = target[y] as Row;
      if (row === base[y]) {
        row = { ch: row.ch.slice(), st: row.st.slice() };
        target[y] = row;
      }
      put(row, o.x, src, this.columns);
    }
  }

  /**
   * Draw cells over the frame at a place, between two frames: an animation
   * step. Only the cells that differ from the screen are written. The
   * cursor and its style are saved before and restored after (DECSC, DECRC).
   * Returns whether a cell changed.
   */
  patch(o: Overlay): boolean {
    if (this.stopped || this.waiting || this.columns === 0) return false;
    // A frame that waits comes first: the patch belongs on top of it.
    if (this.pending !== null) this.flush();
    let out = '';
    const pen: Pen = { cur: null };
    for (let dy = 0; dy < o.rows.length; dy++) {
      const y = o.y + dy;
      if (y < 0 || y >= this.rows) continue;
      const shown = this.shown[y] as Row;
      const src = o.rows[dy] as Row;
      let same = true;
      for (let dx = 0; dx < src.ch.length && same; dx++) {
        const x = o.x + dx;
        if (x >= 0 && x < this.columns && (shown.ch[x] !== src.ch[dx] || shown.st[x] !== src.st[dx])) same = false;
      }
      if (same) continue;
      const row: Row = { ch: shown.ch.slice(), st: shown.st.slice() };
      put(row, o.x, src, this.columns);
      for (const r of runsOf(shown, row, this.columns, pen)) {
        out += `\u001b[${y + 1};${r.x + 1}H${r.text}`;
        this.stats.cells += r.n;
      }
      this.shown[y] = row;
    }
    if (out === '') return false;
    this.send(`\u001b7${out}${RESET}\u001b8`);
    return true;
  }

  private send(s: string): void {
    this.stats.bytes += s.length;
    if (!this.o.write(s)) this.waiting = true;
  }

  /** What the terminal shows now: the text of each row (tests). */
  text(): string[] {
    return this.shown.map((r) => r.ch.join('').trimEnd());
  }

  /** For good: nothing more is written (the program ends). */
  stop(): void {
    this.stopped = true;
    this.pending = null;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }
}
