// The canvas of the world: pixels as half blocks, text over them, and the
// same scene without art, where no colour code is written at all.
import { describe, expect, test } from 'vitest';
import { bitmap, Canvas, mix, rowText, shrink } from '../src/world/canvas.js';
import { parseLine } from '../src/screen.js';
import type { Rgb } from '../src/sprites.js';

const R: Rgb = [255, 0, 0];
const G: Rgb = [0, 255, 0];
const B: Rgb = [0, 0, 255];

describe('pixels', () => {
  test('a picture at twice its size: each pixel of the art is a square of two, and text still counts cells', () => {
    const c = new Canvas(4, 2, { mode: 'truecolor', scale: 2 });
    expect([c.scale, c.pw, c.ph]).toEqual([2, 2, 2]);
    c.pixel(0, 0, R);
    c.blit(bitmap(['g'], { g: G }), 1, 1);
    // Outside the picture: nothing.
    c.pixel(2, 0, B);
    c.fill(0, 2, 2, 1, B);
    expect(c.pixelAt(0, 0)).toEqual(R);
    expect(c.pixelAt(1, 1)).toEqual(G);
    expect(c.pixelAt(1, 0)).toBeNull();
    const rows = c.rows();
    // A whole cell has one colour: a blank with a background, no half block.
    expect(rows.map((r) => r.ch.join(''))).toEqual(['    ', '    ']);
    expect(rows[0]?.st).toEqual(['48;2;255;0;0', '48;2;255;0;0', '', '']);
    expect(rows[1]?.st).toEqual(['', '', '48;2;0;255;0', '48;2;0;255;0']);
    c.text(3, 1, 'x');
    expect(c.rows()[1]?.ch).toEqual([' ', ' ', ' ', 'x']);
    // An odd width: the last column has no pixel of the art.
    expect(new Canvas(5, 1, { mode: 'truecolor', scale: 2 }).pw).toBe(2);
    expect(new Canvas(5, 1, { mode: 'truecolor' }).scale).toBe(1);
  });

  test('words on the art are found: on a thing they clash, on the ground and beside the art they do not', () => {
    const c = new Canvas(6, 2, { mode: 'truecolor' });
    c.setKind('ground');
    c.fill(0, 0, 6, 2, G);
    c.setKind('art');
    c.fill(2, 0, 2, 4, R);
    c.text(0, 0, 'ab', { bg: G });
    c.text(0, 1, 'cdef');
    expect(c.clashes()).toEqual([
      { x: 2, y: 1 },
      { x: 3, y: 1 },
    ]);
    // A thing that is taken away again is no thing, and a change of colour keeps what a pixel is.
    c.fill(2, 2, 1, 2, null);
    c.recolor(0, 0, 6, 4, () => B);
    expect(c.clashes()).toEqual([{ x: 3, y: 1 }]);
    c.clearText(0, 1, 6, 1);
    expect(c.clashes()).toEqual([]);
    // Without art there are no pixels: nothing can clash.
    const t = new Canvas(4, 1, { mode: 'none' });
    t.fill(0, 0, 4, 2, R);
    t.text(0, 0, 'abcd');
    expect(t.clashes()).toEqual([]);
  });

  test('two pixels in a cell: the half block with both colours, one colour as a background', () => {
    const c = new Canvas(4, 1, { mode: 'truecolor' });
    c.pixel(0, 0, R);
    c.pixel(0, 1, G);
    c.pixel(1, 0, R);
    c.pixel(2, 1, B);
    c.fill(3, 0, 1, 2, G);
    const [row] = c.rows();
    expect(row?.ch).toEqual(['▀', '▀', '▄', ' ']);
    expect(row?.st).toEqual(['38;2;255;0;0;48;2;0;255;0', '38;2;255;0;0', '38;2;0;0;255', '48;2;0;255;0']);
  });

  test('the 256 colours: no 24-bit code', () => {
    const c = new Canvas(1, 1, { mode: '256' });
    c.pixel(0, 0, R);
    c.pixel(0, 1, G);
    expect(c.rows()[0]?.st).toEqual(['38;5;196;48;5;46']);
  });

  test('a picture from letters, drawn at a place; transparent pixels leave what is there', () => {
    const b = bitmap(['r.', '.g'], { r: R, g: G });
    expect(b).toMatchObject({ w: 2, h: 2 });
    const c = new Canvas(3, 1, { mode: 'truecolor' });
    c.fill(0, 0, 3, 2, B);
    c.blit(b, 1, 0);
    expect(c.pixelAt(1, 0)).toEqual(R);
    expect(c.pixelAt(2, 0)).toEqual(B);
    expect(c.pixelAt(2, 1)).toEqual(G);
    // Off the canvas: cut, no error.
    c.blit(b, -1, -1);
    c.blit(b, 2, 1);
    expect(c.pixelAt(0, 0)).toEqual(G);
  });

  test('a small picture keeps the colour that most of its pixels have', () => {
    // The last block has one pixel of four: under a third, so it is transparent.
    const big = bitmap(['rrgg', 'rgrg', 'r...', '....'], { r: R, g: G });
    const s = shrink(big, 2);
    expect([s.w, s.h]).toEqual([2, 2]);
    expect(s.px).toEqual([R, G, null, null]);
    expect(mix([0, 0, 0], [100, 200, 50], 0.5)).toEqual([50, 100, 25]);
  });
});

describe('text', () => {
  test('text lies on the pixel under it', () => {
    const c = new Canvas(4, 1, { mode: 'truecolor' });
    c.fill(0, 0, 2, 2, B);
    c.text(0, 0, 'ab c', { fg: R, bold: true });
    const [row] = c.rows();
    expect(row?.ch).toEqual(['a', 'b', ' ', 'c']);
    expect(row?.st).toEqual(['1;38;2;255;0;0;48;2;0;0;255', '1;38;2;255;0;0;48;2;0;0;255', '', '1;38;2;255;0;0']);
  });

  test('the cells are the cells that the painter reads from the same text', () => {
    const c = new Canvas(8, 1, { mode: 'truecolor' });
    c.fill(0, 0, 3, 2, B);
    c.pixel(4, 0, R);
    c.text(1, 0, 'x', { fg: G, inverse: true });
    c.text(5, 0, 'yz', { dim: true, underline: true });
    const [row] = c.rows();
    const parsed = parseLine(rowText(row as never), 8);
    expect(parsed.ch).toEqual(row?.ch);
    expect(parsed.st).toEqual(row?.st);
  });

  test('a wide character keeps two cells, and the edge cuts whole characters', () => {
    const c = new Canvas(5, 1, { mode: 'none' });
    expect(c.text(0, 0, 'a漢b漢')).toBe(4);
    expect(c.rows()[0]?.ch).toEqual(['a', '漢', '', 'b', ' ']);
    expect(c.lines()).toEqual(['a漢b']);
  });

  test('no escape sequence and no invisible character of a run reaches a cell', () => {
    const c = new Canvas(20, 1, { mode: 'none' });
    c.text(0, 0, 'ev\u001b[31mil\u0007 ‮name\u001b]0;x\u0007!');
    expect(c.lines()).toEqual(['evil name!']);
  });
});

describe('without art', () => {
  test('NO_COLOR: no pixel and no colour code, the attributes carry the roles', () => {
    const c = new Canvas(6, 2, { mode: 'none' });
    expect(c.art).toBe(false);
    c.fill(0, 0, 6, 4, R);
    c.text(0, 0, 'amber', { fg: [255, 184, 48], bold: true });
    c.text(0, 1, 'dim', { dim: true, inverse: true });
    const rows = c.rows();
    expect(rows[0]?.st.slice(0, 5)).toEqual(['1', '1', '1', '1', '1']);
    expect(rows[1]?.st.slice(0, 3)).toEqual(['2;7', '2;7', '2;7']);
    const text = rows.map(rowText).join('\n');
    expect(text).not.toMatch(/\u001b\[[\d;]*(3[0-8]|4[0-8]|9\d|10\d)[;m]/);
    expect(text).toContain('amber');
  });

  test('--plain: no escape sequence at all', () => {
    const c = new Canvas(6, 1, { mode: 'none', plain: true });
    c.text(0, 0, 'word', { bold: true, inverse: true, fg: R });
    expect(c.rows().map(rowText)).toEqual(['word  ']);
  });
});
