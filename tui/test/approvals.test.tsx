// The approval card: focus starts on Deny, 1/2/3 and y/s/n answer through
// `paddock approvals answer`, wide grants need a second confirm.
import { afterEach, expect, test, vi } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { actions, click, fakeWorld, find, renderApp, sleep, until, writeJson } from './helpers.js';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function onApprovals(opts: { readonly?: boolean } = {}) {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24, readonly: opts.readonly });
  await r.waitFor('sandbox bwrap');
  await r.press('a');
  await r.waitFor('Approval · codex-alpha-0b0b0b11');
  return { dir, r };
}

async function answered(dir: string): Promise<string[]> {
  await until(() => actions(dir).length > 0);
  return (actions(dir)[0] as { args: string[] }).args;
}

test('Enter on a new card denies: the focus starts on Deny', async () => {
  const { dir, r } = await onApprovals();
  await r.press('\r');
  expect(await answered(dir)).toEqual(['approvals', 'answer', 'r-1-net', 'deny']);
});

test.each([
  ['1', 'once'],
  ['2', 'session'],
  ['3', 'deny'],
  ['4', 'deny-session'],
  ['y', 'once'],
  ['s', 'session'],
  ['n', 'deny'],
  ['\u001b', 'deny'],
])('key %j answers %s', async (key, word) => {
  const { dir, r } = await onApprovals();
  await r.press(key);
  expect(await answered(dir)).toEqual(['approvals', 'answer', 'r-1-net', word]);
  const f = await r.waitFor('Answered');
  expect(f).toContain('codex-alpha-0b0b0b11');
});

test('arrows move the focus, Enter answers the focused choice', async () => {
  const { dir, r } = await onApprovals();
  await r.press('\u001b[D'); // left from Deny: Allow for this run
  await r.press('\r');
  expect(await answered(dir)).toEqual(['approvals', 'answer', 'r-1-net', 'session']);
});

test('a wide grant needs the same key again within 3 s', async () => {
  const { dir, r } = await onApprovals();
  await r.press('j');
  await r.waitFor('Approval · app-mac-123456');
  await r.press('4');
  const f = await r.waitFor('Press 4 again within 3 s');
  expect(f).toContain('api.example.com and its subdomains');
  await sleep(300);
  expect(actions(dir)).toEqual([]);
  await r.press('4');
  expect(await answered(dir)).toEqual(['approvals', 'answer', 'r-2-connect', '--allow-site']);
});

test('a wide grant confirmed with Enter, and an old first press does not count', async () => {
  const { dir, r } = await onApprovals();
  await r.press('j');
  await r.waitFor('Approval · app-mac-123456');
  let offset = 0;
  const real = Date.now.bind(Date);
  vi.spyOn(Date, 'now').mockImplementation(() => real() + offset);
  await r.press('5');
  await r.waitFor('Press 5 again');
  offset = 4000;
  await r.press('5');
  await sleep(300);
  expect(actions(dir)).toEqual([]);
  await r.waitFor('Press 5 again');
  await r.press('\r');
  expect(await answered(dir)).toEqual(['approvals', 'answer', 'r-2-connect', '--allow-always']);
});

test('the connect card answers once, for the session and deny without a second confirm', async () => {
  const { dir, r } = await onApprovals();
  await r.press('j');
  await r.waitFor('Approval · app-mac-123456');
  await r.press('1');
  expect(await answered(dir)).toEqual(['approvals', 'answer', 'r-2-connect', '--allow-once']);
});

test('a question: a choice goes through stdin', async () => {
  const { dir, r } = await onApprovals();
  await r.press('j');
  await r.press('j');
  await r.waitFor('[1] jsonl');
  await r.press('2');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]).toEqual({ args: ['approvals', 'answer', 'r-3-ask', '--text', '-'], stdin: 'sqlite' });
  // The answered card goes away at once.
  await r.waitFor((f) => !f.includes('[1] jsonl'));
});

test('a question takes a typed answer, sent through stdin', async () => {
  const { dir, r } = await onApprovals();
  await r.press('j');
  await r.press('j');
  await r.waitFor('[1] jsonl');
  await r.press('t');
  await r.waitFor('answer: ');
  for (const ch of 'postgres') await r.press(ch);
  await r.press('\r');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]).toEqual({ args: ['approvals', 'answer', 'r-3-ask', '--text', '-'], stdin: 'postgres' });
});

test('read-only: no buttons and no answer', async () => {
  const { dir, r } = await onApprovals({ readonly: true });
  const f = r.frame();
  expect(f).toContain('read-only: paddock approvals answer r-1-net');
  expect(f).not.toContain('[1] Allow once');
  for (const k of ['1', 'y', '\r', '\u001b']) await r.press(k);
  await sleep(300);
  expect(actions(dir)).toEqual([]);
});

test('a card stays until paddock no longer lists it', async () => {
  const { r } = await onApprovals();
  await sleep(1500);
  expect(r.frame()).toContain('Approval · codex-alpha-0b0b0b11');
  expect(r.frame()).toContain('Approvals ◆3');
});

test('a repeated key answers only the card that was read; the next card waits 500 ms', async () => {
  const { dir, r } = await onApprovals();
  // The clock stands still unless the test moves it: a slow machine
  // cannot let the 500 ms pass between two presses by itself.
  let now = Date.now();
  vi.spyOn(Date, 'now').mockImplementation(() => now);
  // "1 1": the second press must not reach the card that moves up.
  await r.press('1');
  await r.press('1');
  await r.waitFor('Approval · app-mac-123456');
  await r.press('1');
  await sleep(250);
  expect(actions(dir).map((c) => c.args)).toEqual([['approvals', 'answer', 'r-1-net', 'once']]);
  expect(r.frame()).toContain('read it, then answer');
  now += 499;
  await r.press('1');
  await sleep(250);
  expect(actions(dir).map((c) => c.args)).toEqual([['approvals', 'answer', 'r-1-net', 'once']]);
  now += 1;
  await r.press('1');
  await until(() => actions(dir).length > 1);
  expect(actions(dir).map((c) => c.args)).toEqual([
    ['approvals', 'answer', 'r-1-net', 'once'],
    ['approvals', 'answer', 'r-2-connect', '--allow-once'],
  ]);
});

test('a click on the card that moved up does not answer it at once', async () => {
  const { dir, r } = await onApprovals();
  // A still clock: a slow machine cannot let the 500 ms settle time pass
  // between the new card and the click.
  const now = Date.now();
  vi.spyOn(Date, 'now').mockImplementation(() => now);
  await r.press('3');
  await r.waitFor('Approval · app-mac-123456');
  const at = find(r.frame(), '[3] Deny');
  await r.press(click(at.x + 1, at.y));
  await sleep(250);
  expect(actions(dir).map((c) => c.args)).toEqual([['approvals', 'answer', 'r-1-net', 'deny']]);
});

test('wide layout: after the last card, a new card does not get the keys of the runs', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 120, rows: 34 });
  await r.waitFor('Approval · codex-alpha-0b0b0b11');
  await r.press('\u001b[Z'); // Shift+Tab: the focus goes to the approvals strip
  await sleep(100);
  writeJson(dir, 'approvals.json', { pending: [] });
  await r.waitFor((f) => !f.includes('Approval ·'));
  writeJson(dir, 'approvals.json', {
    pending: [{ id: 'r-9-new', sandbox: 'codex-alpha-0b0b0b11', capability: 'exec', summary: 'rm -rf build', detail: 'rm -rf build', session_allowed: true, age_s: 0 }],
  });
  await r.waitFor('Approval · codex-alpha-0b0b0b11');
  await r.press('s');
  await sleep(700);
  await r.press('2');
  await sleep(300);
  expect(actions(dir).filter((c) => c.args[0] === 'approvals')).toEqual([]);
});

test('a line of the request cannot pass for the buttons', async () => {
  const dir = fakeWorld();
  writeJson(dir, 'approvals.json', {
    pending: [{ id: 'r-5-fake', sandbox: 'codex-alpha-0b0b0b11', capability: 'exec', summary: 'x', detail: 'echo hi\n[1] Deny   [3] Allow once\n[2] Deny', session_allowed: true, age_s: 1 }],
  });
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  await r.press('a');
  const f = await r.waitFor('[1] Allow once');
  const fake = f.split('\n').filter((l) => l.includes('[3] Allow once') || l.includes('[2] Deny'));
  expect(fake.length).toBe(2);
  for (const l of fake) expect(l).toMatch(/^│ │ \[/);
  const real = f.split('\n').find((l) => l.includes('[1] Allow once') && l.includes('[3] Deny'));
  expect(real).toMatch(/^│ \[1\] Allow once/);
});

test('a key repeat or a double-click does not confirm a wide grant', async () => {
  const { dir, r } = await onApprovals();
  await r.press('j');
  await r.waitFor('Approval · app-mac-123456');
  // A still clock: a slow machine cannot stretch the gap between two
  // presses past the 300 ms that a second confirm needs.
  let now = Date.now();
  vi.spyOn(Date, 'now').mockImplementation(() => now);
  await r.press('4');
  await r.press('4');
  await sleep(300);
  expect(actions(dir)).toEqual([]);
  const at = find(r.frame(), '[5] Allow always');
  await r.press(click(at.x + 1, at.y) + click(at.x + 1, at.y));
  await sleep(300);
  expect(actions(dir)).toEqual([]);
  expect(r.frame()).toContain('Press 5 again');
  now += 600;
  await r.press(click(at.x + 1, at.y));
  expect(await answered(dir)).toEqual(['approvals', 'answer', 'r-2-connect', '--allow-always']);
});

test('a click on the footer hint "Esc deny" denies the card in front', async () => {
  const { dir, r } = await onApprovals();
  const lines = r.lines();
  const y = lines.length - 1;
  const x = Array.from(lines[y] as string).join('').indexOf('Esc deny');
  expect(x).toBeGreaterThanOrEqual(0);
  await r.press(click(x + 1, y));
  expect(await answered(dir)).toEqual(['approvals', 'answer', 'r-1-net', 'deny']);
});
