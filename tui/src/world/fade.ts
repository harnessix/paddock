// The cross-fade of Reduce Motion: one frame in which each cell of the
// picture is between what it was and what it is. Nothing moves. The
// cells of the art are mixed pixel by pixel. A cell of words takes the
// new word at once, on a ground that is mixed.
//
// The mix needs the 24-bit colours. With the 256 colours a cell cannot be
// mixed, and the new picture shows at once.

import type { Row } from '../screen.js';
import type { Rgb } from '../sprites.js';
import { mix } from './canvas.js';

interface Parsed {
  /** The codes that are not a colour: bold, dim, inverse, underline. */
  marks: string[];
  fg: Rgb | null;
  bg: Rgb | null;
}

/** The colours of a style, or null when it has a colour that is not 24-bit. */
function parse(st: string): Parsed | null {
  const out: Parsed = { marks: [], fg: null, bg: null };
  if (st === '') return out;
  const t = st.split(';');
  for (let i = 0; i < t.length; i++) {
    const k = t[i] as string;
    if (k === '38' || k === '48') {
      if (t[i + 1] !== '2') return null;
      const c: Rgb = [Number(t[i + 2]), Number(t[i + 3]), Number(t[i + 4])];
      if (k === '38') out.fg = c;
      else out.bg = c;
      i += 4;
    } else {
      const n = Number(k);
      // One of the terminal's 16 colours: not a colour to mix.
      if ((n >= 30 && n <= 37) || (n >= 40 && n <= 47) || (n >= 90 && n <= 107)) return null;
      out.marks.push(k);
    }
  }
  return out;
}

const fg = (c: Rgb): string => `38;2;${c[0]};${c[1]};${c[2]}`;
const bg = (c: Rgb): string => `48;2;${c[0]};${c[1]};${c[2]}`;

/** A colour between two. One of them can be the terminal's own background, which has no number: then the nearer one shows. */
function between(a: Rgb | null, b: Rgb | null, t: number): Rgb | null {
  if (a && b) return mix(a, b, t);
  return t >= 0.5 ? b : a;
}

/** The two pixels of a cell: its top and its bottom. A cell of words has the pixels of its ground. */
function halves(ch: string, p: Parsed): { top: Rgb | null; bottom: Rgb | null; text: boolean } {
  if (ch === '▀') return { top: p.fg, bottom: p.bg, text: false };
  if (ch === '▄') return { top: p.bg, bottom: p.fg, text: false };
  return { top: p.bg, bottom: p.bg, text: ch !== ' ' };
}

function same(a: Rgb, b: Rgb): boolean {
  return a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

/**
 * The picture between `from` and `to`: t = 0 is `from`, t = 1 is `to`.
 * The two must have one size. If not, the result is `to`.
 */
export function crossfade(from: readonly Row[], to: readonly Row[], t: number): Row[] {
  if (t >= 1 || from.length !== to.length) return to as Row[];
  return to.map((b, y) => {
    const a = from[y] as Row;
    if (a.ch.length !== b.ch.length) return b;
    const row: Row = { ch: b.ch.slice(), st: b.st.slice() };
    for (let x = 0; x < b.ch.length; x++) {
      const ch = b.ch[x] as string;
      const st = b.st[x] as string;
      if ((a.ch[x] === ch && a.st[x] === st) || ch === '') continue;
      const pa = parse(a.st[x] as string);
      const pb = parse(st);
      if (!pa || !pb) continue;
      const ha = halves(a.ch[x] as string, pa);
      const hb = halves(ch, pb);
      if (hb.text) {
        // The new word, on a ground between the old and the new one.
        const ground = between(ha.bottom ?? ha.top, pb.bg, t);
        const ink = pb.fg && ground ? mix(ground, pb.fg, t) : pb.fg;
        row.st[x] = [...pb.marks, ...(ink ? [fg(ink)] : []), ...(ground ? [bg(ground)] : [])].join(';');
        continue;
      }
      const top = between(ha.top, hb.top, t);
      const bottom = between(ha.bottom, hb.bottom, t);
      if (top && bottom) {
        row.ch[x] = same(top, bottom) ? ' ' : '▀';
        row.st[x] = same(top, bottom) ? bg(top) : `${fg(top)};${bg(bottom)}`;
      } else if (top) {
        row.ch[x] = '▀';
        row.st[x] = fg(top);
      } else if (bottom) {
        row.ch[x] = '▄';
        row.st[x] = fg(bottom);
      } else {
        row.ch[x] = ' ';
        row.st[x] = '';
      }
    }
    return row;
  });
}
