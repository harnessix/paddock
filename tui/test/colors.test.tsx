// Colour roles: the 16 ANSI colours only, a different accent in light and
// dark themes, the hover underline, and plain mode without colours.
import { afterEach, beforeAll, expect, test } from 'vitest';

let h: typeof import('./helpers.js');
let lib: typeof import('ink-testing-library');

beforeAll(async () => {
  // Colours are chosen when chalk loads: force them before the imports.
  process.env.FORCE_COLOR = '1';
  lib = await import('ink-testing-library');
  h = await import('./helpers.js');
});
afterEach(() => lib.cleanup());

// SGR codes of the 16 colours (30-37, 90-97 and their backgrounds) and attributes.
function sgrCodes(frame: string): number[] {
  const out: number[] = [];
  for (const m of frame.matchAll(/\u001b\[([\d;]*)m/g)) for (const c of (m[1] ?? '').split(';')) if (c) out.push(Number(c));
  return out;
}

test('dark and light themes use only the 16 colours, with their own accent and warning', async () => {
  h.fakeWorld();
  const dark = await h.renderApp({ columns: 80, rows: 24, theme: 'dark' });
  const fd = await dark.waitFor('sandbox bwrap');
  dark.unmount();
  const light = await h.renderApp({ columns: 80, rows: 24, theme: 'light' });
  const fl = await light.waitFor('sandbox bwrap');
  for (const f of [fd, fl]) {
    expect(f).not.toMatch(/\u001b\[[\d;]*[34]8;[25];/); // no 256-colour or RGB
    for (const c of sgrCodes(f)) expect(c >= 0 && c <= 107, String(c)).toBe(true);
  }
  expect(sgrCodes(fd)).toContain(33); // dark warning: yellow
  expect(sgrCodes(fd)).toContain(36); // dark accent: cyan
  expect(sgrCodes(fl)).toContain(35); // light warning: magenta
  expect(sgrCodes(fl)).toContain(34); // light accent: blue
  expect(sgrCodes(fl)).not.toContain(33);
});

test('hover underlines the item under the pointer', async () => {
  h.fakeWorld();
  const r = await h.renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  const plain = r.frame().replace(/\u001b\[[\d;]*m/g, '');
  const at = h.find(plain, 'Board');
  const before = (r.frame().match(/\u001b\[4m/g) ?? []).length;
  await r.press(`\u001b[<35;${at.x + 2};${at.y + 1}M`);
  await r.waitFor((f) => (f.match(/\u001b\[4m/g) ?? []).length > before);
  await r.press('\u001b[<35;2;20M');
  await r.waitFor((f) => (f.match(/\u001b\[4m/g) ?? []).length === before);
});

test('plain mode has no colours', async () => {
  h.fakeWorld();
  const r = await h.renderApp({ columns: 80, rows: 24, plain: true, mouse: false });
  const f = await r.waitFor('sandbox bwrap');
  expect(sgrCodes(f).filter((c) => (c >= 30 && c <= 37) || (c >= 90 && c <= 97))).toEqual([]);
});
