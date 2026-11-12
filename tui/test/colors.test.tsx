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

function strip(s: string): string {
  return s.replace(/\u001b\[[\d;]*m/g, '');
}

/** The label stands in a style in the frame: the code that turns it on comes after the one that turns it off. */
function styled(frame: string, label: string, on: number, off: number): boolean {
  const at = frame.indexOf(label);
  if (at < 0) return false;
  const line = frame.slice(frame.lastIndexOf('\n', at) + 1, at);
  return line.lastIndexOf(`\u001b[${on}m`) > Math.max(line.lastIndexOf(`\u001b[${off}m`), line.lastIndexOf('\u001b[0m'));
}

const inverse = (frame: string, label: string): boolean => styled(frame, label, 7, 27);
const underlined = (frame: string, label: string): boolean => styled(frame, label, 4, 24);

test('a button shows the press at once, and a drag off the button lets it go', async () => {
  const dir = h.fakeWorld();
  const r = await h.renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  await r.press('a');
  await r.waitFor('[1] Allow once');
  const at = h.find(strip(r.frame()), '[1] Allow once');
  expect(inverse(r.frame(), '[1] Allow once')).toBe(false);
  // The button goes down: inverse video, and no answer yet.
  await r.press(`\u001b[<0;${at.x + 3};${at.y + 1}M`);
  await r.waitFor((f) => inverse(f, '[1] Allow once'));
  expect(h.actions(dir)).toEqual([]);
  // The pointer leaves with the button held: the button comes up, and the release answers nothing.
  await r.press(`\u001b[<32;${at.x + 3};${at.y + 4}M`);
  await r.waitFor((f) => !inverse(f, '[1] Allow once'));
  await r.press(`\u001b[<0;${at.x + 3};${at.y + 4}m`);
  await h.rounds(dir, 1);
  expect(h.actions(dir)).toEqual([]);
  // Down and up on the button: the answer.
  await r.press(`\u001b[<0;${at.x + 3};${at.y + 1}M`);
  await r.waitFor((f) => inverse(f, '[1] Allow once'));
  await r.press(`\u001b[<0;${at.x + 3};${at.y + 1}m`);
  await h.until(() => h.actions(dir).length > 0);
  expect(h.actions(dir)[0]?.args).toEqual(['approvals', 'answer', 'r-1-net', 'once']);
});

test('a room lights up under the pointer, and nothing stays lit when the terminal loses the focus', async () => {
  h.fakeWorld();
  const { ArtLayer } = await import('../src/art-layer.js');
  const art = new ArtLayer({ mode: 'truecolor', sink: null, screen: () => ({ columns: 100, rows: 30 }) });
  const r = await h.renderApp({ columns: 100, rows: 30, art });
  await r.waitFor('1/7');
  const lit = (f: string): boolean => underlined(f, 'codex-0b0b0b');
  expect(lit(r.frame())).toBe(false);
  const at = h.find(strip(r.frame()), 'codex-0b0b0b');
  // Anywhere on the card, also on its character.
  await r.press(`\u001b[<35;${at.x + 4};${at.y + 6}M`);
  await r.waitFor(lit);
  // The selected card has the focus colour already: it does not change.
  const sel = h.find(strip(r.frame()), '› claude-71c3ac');
  await r.press(`\u001b[<35;${sel.x + 4};${sel.y + 6}M`);
  await r.waitFor((f) => !lit(f));
  expect(underlined(r.frame(), 'claude-71c3ac')).toBe(false);
  await r.press(`\u001b[<35;${at.x + 4};${at.y + 6}M`);
  await r.waitFor(lit);
  await r.press('\u001b[O');
  await r.waitFor((f) => !lit(f));
});

test('NO_COLOR: no colour code at all, and bold, dim and inverse carry the roles', async () => {
  const { makeTheme, noColor, roleProps } = await import('../src/theme.js');
  expect(noColor({ NO_COLOR: '1' })).toBe(true);
  expect(noColor({ NO_COLOR: '' })).toBe(false);
  expect(noColor({})).toBe(false);
  const mono = makeTheme('dark', false, true);
  expect(roleProps(mono, 'warning')).toEqual({ bold: true });
  expect(roleProps(mono, 'error')).toEqual({ bold: true });
  expect(roleProps(mono, 'muted')).toEqual({ dimColor: true });
  expect(roleProps(mono, 'success')).toEqual({});
  h.fakeWorld();
  const r = await h.renderApp({ columns: 80, rows: 24, mono: true });
  await r.waitFor('sandbox bwrap');
  const colours = (f: string): number[] => sgrCodes(f).filter((c) => (c >= 30 && c <= 37) || (c >= 40 && c <= 47) || (c >= 90 && c <= 107) || c === 38 || c === 48);
  expect(colours(r.frame())).toEqual([]);
  // The boxes stay, the selection is inverse, the muted words are dim.
  expect(r.frame()).toContain('╭─ ');
  expect(sgrCodes(r.frame())).toContain(7);
  expect(sgrCodes(r.frame())).toContain(2);
  await r.press('a');
  const card = await r.waitFor('[1] Allow once');
  expect(colours(card)).toEqual([]);
  // The pill and the focused button are inverse; the command has its gutter, and no dark box.
  expect(inverse(card, '◆ Approval needed')).toBe(true);
  expect(strip(card)).toContain('│ exec: git push');
});
