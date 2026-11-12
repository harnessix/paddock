// The mouse: SGR reports on stdin. A click on a button answers, a click
// elsewhere does nothing, the wheel scrolls the pane under the pointer,
// and with the mouse off nothing reacts.
import { afterEach, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { parseMouse } from '../src/mouse.js';
import { actions, click, fakeWorld, find, renderApp, sleep, until, writeJson } from './helpers.js';

afterEach(() => cleanup());

test('the parser reads SGR presses, releases, wheel and motion', () => {
  expect(parseMouse('[<0;10;5M')).toEqual({ kind: 'press', button: 0, x: 9, y: 4, shift: false });
  expect(parseMouse('\u001b[<0;10;5m')).toMatchObject({ kind: 'release' });
  expect(parseMouse('[<64;1;1M')).toMatchObject({ kind: 'wheelUp' });
  expect(parseMouse('[<65;1;1M')).toMatchObject({ kind: 'wheelDown' });
  expect(parseMouse('[<35;3;4M')).toMatchObject({ kind: 'move', x: 2, y: 3 });
  expect(parseMouse('[<32;3;4M')).toMatchObject({ kind: 'drag' });
  expect(parseMouse('[<4;3;4M')).toMatchObject({ shift: true });
  expect(parseMouse('[A')).toBeNull();
  expect(parseMouse('[<0;0;5M')).toBeNull();
  // 128 and up: the back and forward buttons. They are not a left press.
  expect(parseMouse('[<128;10;5M')).toBeNull();
  expect(parseMouse('[<129;10;5m')).toBeNull();
});

async function onApprovals(mouse: boolean) {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24, mouse });
  await r.waitFor('sandbox bwrap');
  await r.press('a');
  await r.waitFor('[1] Allow once');
  return { dir, r };
}

test('a click on a button answers', async () => {
  const { dir, r } = await onApprovals(true);
  const at = find(r.frame(), '[1] Allow once');
  await r.press(click(at.x + 2, at.y));
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['approvals', 'answer', 'r-1-net', 'once']);
});

test('a click outside every button does nothing', async () => {
  const { dir, r } = await onApprovals(true);
  const req = find(r.frame(), 'exec: git push');
  await r.press(click(req.x + 5, req.y));
  const blank = find(r.frame(), '[4] Deny for this run');
  await r.press(click(blank.x + 30, blank.y));
  await r.press(click(2, 0));
  await sleep(400);
  expect(actions(dir)).toEqual([]);
  expect(r.frame()).toContain('Approval · codex-alpha-0b0b0b11');
});

test('with the mouse off, clicks do nothing and never act as keys; M turns it on', async () => {
  const { dir, r } = await onApprovals(false);
  const at = find(r.frame(), '[1] Allow once');
  await r.press(click(at.x + 2, at.y));
  await r.press('\u001b[<64;5;5M');
  await sleep(400);
  expect(actions(dir)).toEqual([]);
  expect(r.frame()).toContain('M mouse on');
  await r.press('M');
  await r.waitFor('hold Shift to select text');
  await r.press(click(at.x + 2, at.y));
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['approvals', 'answer', 'r-1-net', 'once']);
});

test('tabs click, a double-click opens a run', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  const tab = find(r.frame(), 'Board');
  await r.press(click(tab.x + 1, tab.y));
  await r.waitFor('◇ open');
  const runs = find(r.frame(), 'Runs');
  await r.press(click(runs.x + 1, runs.y));
  await r.waitFor('sandbox podman');
  const row = find(r.frame(), '0f0f0f');
  await r.press(click(row.x, row.y) + click(row.x, row.y));
  await r.waitFor('Run · grok-0f0f0f · sandbox podman');
});

test('the wheel scrolls the pane under the pointer', async () => {
  fakeWorld((dir) => {
    const sandboxes = Array.from({ length: 40 }, (_, i) => ({
      id: `claude-many-${String(i).padStart(6, '0')}aa`,
      short: String(i).padStart(6, '0'),
      harness: 'claude',
      project: '/src/many',
      backend: 'bwrap',
      started: 100 + i,
    }));
    writeJson(dir, 'ps.json', { sandboxes, ended: [] });
    writeJson(dir, 'approvals.json', { pending: [] });
    writeJson(dir, 'tail.json', { lines: Array.from({ length: 60 }, (_, i) => `out ${i}`) });
  });
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('1/40');
  const row = find(r.frame(), '000005');
  await r.press(`\u001b[<65;${row.x + 1};${row.y + 1}M`);
  await r.waitFor('4/40');
  await r.press(`\u001b[<65;${row.x + 1};${row.y + 1}M`);
  await r.waitFor('7/40');
  await r.press(`\u001b[<64;${row.x + 1};${row.y + 1}M`);
  await r.waitFor('4/40');
  // Wheel over the header: no pane, nothing moves.
  await r.press('\u001b[<65;2;1M');
  await sleep(200);
  expect(r.frame()).toContain('4/40');
  // In the detail, the wheel scrolls the output, and End jumps back.
  await r.press('\r');
  await r.waitFor('out 59');
  const out = find(r.frame(), 'out 59');
  await r.press(`\u001b[<64;${out.x + 1};${out.y + 1}M`);
  const f = await r.waitFor('3 up');
  expect(f).not.toContain('out 59');
  expect(f).toContain('End jumps to the bottom');
  await r.press('G');
  await r.waitFor('out 59');
});

test('a click acts on the release: a drag from a button to elsewhere does nothing', async () => {
  const { dir, r } = await onApprovals(true);
  const at = find(r.frame(), '[1] Allow once');
  const req = find(r.frame(), 'exec: git push');
  // Press on the button, drag, release on the request text.
  await r.press(`\u001b[<0;${at.x + 3};${at.y + 1}M`);
  await r.press(`\u001b[<32;${req.x + 3};${req.y + 1}M`);
  await r.press(`\u001b[<0;${req.x + 3};${req.y + 1}m`);
  // Press elsewhere, release on the button.
  await r.press(`\u001b[<0;${req.x + 3};${req.y + 1}M`);
  await r.press(`\u001b[<0;${at.x + 3};${at.y + 1}m`);
  await sleep(300);
  expect(actions(dir)).toEqual([]);
  // A press alone does nothing until its release.
  await r.press(`\u001b[<0;${at.x + 3};${at.y + 1}M`);
  await sleep(300);
  expect(actions(dir)).toEqual([]);
  await r.press(`\u001b[<0;${at.x + 3};${at.y + 1}m`);
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['approvals', 'answer', 'r-1-net', 'once']);
});

test('the back and forward buttons of a mouse never click', async () => {
  const { dir, r } = await onApprovals(true);
  const at = find(r.frame(), '[1] Allow once');
  for (const code of [128, 129, 130, 131]) {
    await r.press(`\u001b[<${code};${at.x + 3};${at.y + 1}M\u001b[<${code};${at.x + 3};${at.y + 1}m`);
  }
  await sleep(300);
  expect(actions(dir)).toEqual([]);
});
