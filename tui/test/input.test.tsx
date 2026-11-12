// No key press is lost: keys that arrive together (fast typing, a key
// that repeats, a paste) all count, one by one, in their order.
import { afterEach, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { actions, fakeWorld, find, renderApp, rounds, until, writeJson } from './helpers.js';
import { splitInput, takeKey, TYPE_AHEAD_MAX, type KeyEvent } from '../src/keys.js';
import type { Key } from 'ink';

afterEach(() => cleanup());

function strip(s: string): string {
  return s.replace(/\u001b\[[\d;]*m/g, '');
}

/** The row of the selected line of a list. */
function selectedRow(f: string): number {
  return find(strip(f), '› ').y;
}

test('letters that arrive in one piece all reach the filter', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  await r.waitFor('sandbox bwrap');
  await r.press('/');
  await r.waitFor('↵ keep');
  await r.press('codex');
  await r.waitFor((f) => strip(f).includes('/ codex'));
  await r.press('\u007f\u007fe-x');
  await r.waitFor((f) => strip(f).includes('/ code-x'));
});

test('the key that opens a line and the text for it, in one piece', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  await r.waitFor('sandbox bwrap');
  await r.press('/beta\r');
  const f = await r.waitFor((x) => x.includes('filter "beta"'));
  expect(strip(f)).toContain('Runs · 1 live, 2 ended');
});

test('arrows and letters that repeat move one row each', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  const first = selectedRow(await r.waitFor('sandbox bwrap'));
  await r.press('\u001b[B\u001b[B\u001b[B');
  await r.waitFor((f) => selectedRow(f) === first + 3);
  await r.press('jjkj');
  await r.waitFor((f) => selectedRow(f) === first + 5);
  await r.press('kk\u001b[A\u001b[A\u001b[A');
  await r.waitFor((f) => selectedRow(f) === first);
});

test('the palette, the form and a typed answer take text in one piece', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  await r.waitFor('sandbox bwrap');
  await r.press(':boa');
  await r.waitFor((f) => strip(f).includes(': boa'));
  await r.press('\u001b');
  await r.waitFor('Runs · 4 live');
  await r.press('n');
  await r.waitFor('New run');
  await r.press('claude\t./some/dir\tprompt.md');
  const f = strip(await r.waitFor((x) => x.includes('prompt.md')));
  expect(f).toContain('claude');
  expect(f).toContain('./some/dir');
  await r.press('\u001b');
  await r.waitFor('Runs · 4 live');
  // A message to a run, typed and sent in one piece.
  await r.press('whello there\r');
  await until(() => actions(dir).some((c) => c.args[0] === 'mail'));
  const sent = actions(dir).find((c) => c.args[0] === 'mail');
  expect(JSON.stringify(sent)).toContain('hello there');
});

const PASTE = (text: string): string => `\u001b[200~${text}\u001b[201~`;

test('a paste goes into the open text field, with its line breaks as spaces', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  await r.waitFor('sandbox bwrap');
  await r.press('/');
  await r.waitFor('↵ keep');
  await r.press(PASTE('co\ndex'));
  await r.waitFor((f) => strip(f).includes('/ co dex'));
});

test('a paste with no text field open does nothing: its letters are not commands', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  await r.waitFor('sandbox bwrap');
  await r.press(PASTE('sy\rQ n b'));
  const f = strip(await r.waitFor('Pasted text goes into a text field'));
  expect(f).toContain('Runs · 4 live, 3 ended');
  expect(f).not.toContain('Stop ');
  await rounds(dir, 1);
  expect(actions(dir)).toEqual([]);
});

test('a terminal that does not mark a paste: at most four command keys from one read', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  const first = selectedRow(await r.waitFor('sandbox bwrap'));
  await r.press('jjjjjjjjjj');
  await r.waitFor((f) => selectedRow(f) === first + TYPE_AHEAD_MAX);
  await rounds(dir, 1);
  expect(selectedRow(r.frame())).toBe(first + TYPE_AHEAD_MAX);
});

test('the question before a stop takes no answer from keys typed ahead', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  await r.waitFor('sandbox bwrap');
  await r.press('sy');
  await r.waitFor('Stop claude-71c3ac?');
  await rounds(dir, 1);
  expect(strip(r.frame())).toContain('Stop claude-71c3ac?');
  expect(actions(dir)).toEqual([]);
  // A y after the question showed stops the run.
  await r.press('y');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args.slice(0, 2)).toEqual(['stop', 'claude-alpha-71c3ac9d']);
});

const NONE = { upArrow: false, downArrow: false, leftArrow: false, rightArrow: false, pageDown: false, pageUp: false, home: false, end: false, return: false, escape: false, ctrl: false, shift: false, tab: false, backspace: false, delete: false, meta: false, super: false, hyper: false, capsLock: false, numLock: false } as Key;

function flags(e: KeyEvent): string {
  const on = Object.entries(e.key)
    .filter(([, v]) => v === true)
    .map(([k]) => k);
  return [JSON.stringify(e.input), ...on].join(' ');
}

test('one read is split into its keys', () => {
  expect(splitInput('/beta\r', NONE).map(flags)).toEqual(['"/beta"', '"\\r" return']);
  expect(splitInput('a\tb\u007f\u0003', NONE).map(flags)).toEqual(['"a"', '"\\t" tab', '"b"', '"" backspace', '"c" ctrl']);
  expect(splitInput('j', NONE).map(flags)).toEqual(['"j"']);
  // One key of Ink stays one key, also with text in it.
  expect(splitInput('ab', { ...NONE, meta: true })).toHaveLength(1);
  // A sequence that Ink does not know is never keys.
  expect(splitInput('[1;5C', NONE)).toEqual([{ input: '[1;5C', key: NONE }]);
  expect(splitInput('[<0;3;4M', NONE)).toHaveLength(1);
  // A line feed is never Enter, and in a long read (a paste) no line break is.
  expect(splitInput('a\nb', NONE).map(flags)).toEqual(['"a b"']);
  expect(splitInput(`${'x'.repeat(80)}\rQ`, NONE).map(flags)).toEqual([`"${'x'.repeat(80)} Q"`]);
});

test('a run of characters is text in a text field, and single command keys elsewhere', () => {
  const q = splitInput('abcdefgh', NONE);
  expect(takeKey([...q], true)?.input).toBe('abcdefgh');
  const taken: string[] = [];
  for (let e = takeKey(q, false); e; e = takeKey(q, false)) taken.push(e.input);
  expect(taken).toEqual(['a', 'b', 'c', 'd']);
  expect(q).toEqual([]);
  // The first key opens a text field: the rest of the run is its text.
  const q2 = splitInput('/beta', NONE);
  expect(takeKey(q2, false)?.input).toBe('/');
  expect(takeKey(q2, true)?.input).toBe('beta');
  expect(takeKey([{ input: 'G', key: NONE, text: true }], false)?.key.shift).toBe(true);
});

test('the selection stays on its run when paddock sorts the runs anew', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  await r.waitFor('sandbox bwrap');
  const selected = (f: string): string => strip(f).split('\n').find((l) => l.includes('› ')) ?? '';
  await r.press('\u001b[B');
  await r.waitFor((f) => selected(f).includes('0b0b0b'));
  const row = selectedRow(r.frame());
  // The card of the first run is answered elsewhere: that run no longer waits, and moves down the list.
  const approvals = JSON.parse(readFileSync(join(dir, 'approvals.json'), 'utf8')) as { pending: { id: string }[] };
  writeJson(dir, 'approvals.json', { ...approvals, pending: approvals.pending.filter((p) => p.id !== 'r-3-ask') });
  await r.waitFor((f) => strip(f).includes('● running'));
  // The selected run is now the first of the list, and it is still the selected one.
  expect(selected(r.frame())).toContain('0b0b0b');
  expect(selectedRow(r.frame())).toBe(row - 1);
  // The next key moves from there.
  await r.press('\u001b[B');
  await r.waitFor((f) => selectedRow(f) === row);
  expect(selected(r.frame())).not.toContain('0b0b0b');
});

test('wheel steps that arrive in one read add up', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 100, rows: 30 });
  const first = selectedRow(await r.waitFor('sandbox bwrap'));
  // Two steps down over the list: three rows each.
  await r.press('\u001b[<65;10;6M\u001b[<65;10;6M');
  await r.waitFor((f) => selectedRow(f) === first + 6);
  await r.press('\u001b[<64;10;6M\u001b[<64;10;6M');
  await r.waitFor((f) => selectedRow(f) === first);
});
