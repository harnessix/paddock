// Text from paddock comes from sandboxes: run names, request text, mail.
// None of it may reach the terminal with escape sequences or control
// characters, so every string passes through these functions before Ink
// draws it.

const ESC = 0x1b;
const CSI_8BIT = 0x9b;
const OSC_8BIT = 0x9d;

function isControl(cp: number): boolean {
  return cp < 0x20 || (cp >= 0x7f && cp <= 0x9f);
}

// Characters that change the direction or the visibility of text, so a
// line can look different from what it holds.
function isInvisible(cp: number): boolean {
  return (
    (cp >= 0x200b && cp <= 0x200f) ||
    (cp >= 0x202a && cp <= 0x202e) ||
    (cp >= 0x2060 && cp <= 0x2064) ||
    (cp >= 0x2066 && cp <= 0x2069) ||
    (cp >= 0x2028 && cp <= 0x2029) ||
    (cp >= 0xfe00 && cp <= 0xfe0f) ||
    (cp >= 0xe0000 && cp <= 0xe007f) ||
    (cp >= 0xe0100 && cp <= 0xe01ef) ||
    cp === 0x061c ||
    cp === 0x115f ||
    cp === 0x1160 ||
    cp === 0x3164 ||
    cp === 0xffa0 ||
    cp === 0xfeff ||
    cp === 0x00ad
  );
}

// Characters that take two cells in a terminal (East Asian wide and
// fullwidth, and the emoji blocks).
function isWide(cp: number): boolean {
  return (
    (cp >= 0x1100 && cp <= 0x115f) ||
    (cp >= 0x2e80 && cp <= 0x303e) ||
    (cp >= 0x3041 && cp <= 0x33ff) ||
    (cp >= 0x3400 && cp <= 0x4dbf) ||
    (cp >= 0x4e00 && cp <= 0x9fff) ||
    (cp >= 0xa000 && cp <= 0xa4cf) ||
    (cp >= 0xa960 && cp <= 0xa97f) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe10 && cp <= 0xfe19) ||
    (cp >= 0xfe30 && cp <= 0xfe6f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1f64f) ||
    (cp >= 0x1f900 && cp <= 0x1f9ff) ||
    (cp >= 0x20000 && cp <= 0x3fffd)
  );
}

const ZERO_WIDTH = /^[\p{Mn}\p{Me}]$/u;
// Emoji that a terminal draws in two cells by default, in every block:
// U+1F680-1F6FF, U+1FA70-1FAFF and the ones in U+2600-27BF such as ⚡ and ✅.
const EMOJI = /^\p{Emoji_Presentation}$/u;

/** The cells that one character takes in a terminal: 0, 1 or 2. */
export function charCells(ch: string): number {
  const cp = ch.codePointAt(0) ?? 0;
  if (cp < 0x20 || (cp >= 0x7f && cp < 0xa0)) return 0;
  if (ZERO_WIDTH.test(ch)) return 0;
  return isWide(cp) || EMOJI.test(ch) ? 2 : 1;
}

/** The cells that a text takes in a terminal. */
export function cells(text: string): number {
  let n = 0;
  for (const ch of text) n += charCells(ch);
  return n;
}

/** The text without escape sequences, control and invisible characters. Tabs become spaces, line breaks stay. */
export function cleanText(input: unknown): string {
  const text = typeof input === 'string' ? input : input == null ? '' : String(input);
  let out = '';
  const cps = Array.from(text);
  for (let i = 0; i < cps.length; i++) {
    const ch = cps[i] as string;
    const cp = ch.codePointAt(0) ?? 0;
    if (cp === ESC) {
      i = skipEscape(cps, i);
      continue;
    }
    if (cp === CSI_8BIT) {
      i = skipCsi(cps, i + 1);
      continue;
    }
    if (cp === OSC_8BIT) {
      i = skipString(cps, i + 1);
      continue;
    }
    if (ch === '\n') {
      out += '\n';
      continue;
    }
    if (ch === '\t') {
      out += '  ';
      continue;
    }
    if (isControl(cp) || isInvisible(cp)) continue;
    out += ch;
  }
  return out;
}

/** One line: line breaks become spaces, runs of spaces shrink, the ends are trimmed. */
export function oneLine(input: unknown): string {
  return cleanText(input).replace(/\s+/g, ' ').trim();
}

/** Cut to `width` terminal cells with an ellipsis at the end. */
export function cut(text: string, width: number): string {
  if (width <= 0) return '';
  if (cells(text) <= width) return text;
  if (width === 1) return '…';
  let out = '';
  let used = 0;
  for (const ch of text) {
    const w = charCells(ch);
    if (used + w > width - 1) break;
    out += ch;
    used += w;
  }
  return out + '…';
}

/** Pad or cut to exactly `width` terminal cells. */
export function fit(text: string, width: number): string {
  const c = cut(text, width);
  const n = cells(c);
  return n < width ? c + ' '.repeat(width - n) : c;
}

/** A column: the text in `width - 1` cells, then one space, so that two columns never touch. */
export function col(text: string, width: number): string {
  return fit(text, width - 1) + ' ';
}

function skipEscape(cps: string[], i: number): number {
  const next = cps[i + 1];
  if (next === '[') return skipCsi(cps, i + 2);
  if (next === ']' || next === 'P' || next === 'X' || next === '^' || next === '_') {
    return skipString(cps, i + 2);
  }
  if (next !== undefined && '()*+-./#%'.includes(next)) return i + 2;
  return next === undefined ? i : i + 1;
}

// Returns the index of the last character of the sequence.
function skipCsi(cps: string[], start: number): number {
  for (let j = start; j < cps.length; j++) {
    const cp = (cps[j] as string).codePointAt(0) ?? 0;
    if (cp >= 0x40 && cp <= 0x7e) return j;
  }
  return cps.length - 1;
}

// OSC, DCS and the other string sequences end with BEL or ST.
function skipString(cps: string[], start: number): number {
  for (let j = start; j < cps.length; j++) {
    const ch = cps[j] as string;
    if (ch === '\u0007' || ch === '\u009c') return j;
    if (ch === '\u001b' && cps[j + 1] === '\\') return j + 1;
  }
  return cps.length - 1;
}
