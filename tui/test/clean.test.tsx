// Control characters in run names, request text and mail never reach the terminal.
import { afterEach, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { cells, cleanText, cut, fit, oneLine } from '../src/clean.js';
import { fakeWorld, renderApp } from './helpers.js';

afterEach(() => cleanup());

test('escape sequences, C0, C1 and invisible characters go', () => {
  expect(cleanText('a\u001b[31mred\u001b[0m b')).toBe('ared b');
  expect(cleanText('x\u001b]0;title\u0007y')).toBe('xy');
  expect(cleanText('x\u001b]8;;http://e\u001b\\link\u001b]8;;\u001b\\y')).toBe('xlinky');
  expect(cleanText('x\u009b2Jy\u009d0;t\u009cz')).toBe('xyz');
  expect(cleanText('a\u0007b\u0008c\u007fd\u0000e')).toBe('abcde');
  expect(cleanText('safe‮evil​⁦x')).toBe('safeevilx');
  expect(cleanText('a\u001bPq#0;payload\u001b\\b')).toBe('ab');
  expect(cleanText('tab\there\nnext')).toBe('tab  here\nnext');
  expect(cleanText('end\u001b')).toBe('end');
  expect(cleanText(undefined)).toBe('');
  expect(oneLine('  two\n lines  ')).toBe('two lines');
});

test('line and paragraph separators, fillers, variation selectors and tags go', () => {
  expect(cleanText('a\u061cb\u2028c\u2029d')).toBe('abcd');
  expect(cleanText('x\u3164y\u115fz\u1160w\uffa0v')).toBe('xyzwv');
  expect(cleanText('ok\ufe0f\ufe00!')).toBe('ok!');
  expect(cleanText('t\u{e0001}\u{e0041}\u{e007f}ag\u{e0100}')).toBe('tag');
});

test('cut and fit count terminal cells: CJK takes two', () => {
  expect(cells('日本語')).toBe(6);
  expect(cut('日本語テキスト', 5)).toBe('日本…');
  expect(cells(cut('日本語テキスト', 6))).toBeLessThanOrEqual(6);
  expect(fit('日本', 6)).toBe('日本  ');
  expect(cells(fit('日本語テキスト', 7))).toBe(7);
  expect(cells('e\u0301')).toBe(1);
});

test('emoji outside the old blocks take two cells too', () => {
  // U+1F680-1F6FF, U+1FA70-1FAFF and U+2600-27BF.
  for (const e of ['🚀', '🛑', '🪲', '🫠', '⚡', '✅', '❌', '⌛']) expect(cells(e), e).toBe(2);
  // Text-style symbols in the same blocks stay one cell.
  for (const t of ['☀', '✓', '✖', '◆', '▲', '●']) expect(cells(t), t).toBe(1);
  expect(cells(fit('deploy 🚀 now', 10))).toBe(10);
  expect(cut('🚀🚀🚀', 5)).toBe('🚀🚀…');
});

test('cut and fit count characters', () => {
  expect(cut('abcdef', 4)).toBe('abc…');
  expect(cut('abc', 4)).toBe('abc');
  expect(fit('ab', 4)).toBe('ab  ');
  expect(cut('◆◆◆◆◆', 3)).toBe('◆◆…');
});

test('no escape from the fixtures reaches the screen of any view', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 160, rows: 48 });
  const screens: string[] = [];
  screens.push(await r.waitFor('evil'));
  for (const k of ['b', 'a', 'm']) {
    await r.press(k);
    await new Promise((res) => setTimeout(res, 200));
    screens.push(r.frame());
  }
  await r.press('\r');
  screens.push(await r.waitFor('Which ledger format?'));
  await r.press('\r');
  screens.push(await r.waitFor('Option two'));
  for (const s of screens) {
    expect(s).not.toMatch(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f‮]/);
    expect(s).not.toContain('pwned');
    expect(s).not.toContain('owned');
  }
  const all = screens.join('\n');
  expect(all).toContain('idle with unread mail');
  expect(all).toContain('git push origin main --force-with-lease');
  expect(all).toContain('Which ledger format?');
});

test('the activity line of a run is cleaned in the detail', async () => {
  const { writeJson } = await import('./helpers.js');
  fakeWorld((dir) => writeJson(dir, 'board.json', { projects: [] }));
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('gamma · linemoved');
  await r.press('j');
  await r.press('j');
  await r.press('\r');
  const f = await r.waitFor('linemoved and RTL');
  expect(f).toContain('Run · evil-0e0e0e');
  expect(f).not.toMatch(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f‮]/);
});

test('the error line of paddock is the clap error, cleaned', async () => {
  const { errorLine } = await import('../src/paddock.js');
  const stderr = "error: unrecognized subcommand '\u001b[33mboard\u001b[0m'\n\nUsage: paddock <COMMAND>\n\nFor more information, try '--help'.\n";
  expect(errorLine({ code: 2, stdout: '', stderr })).toBe("error: unrecognized subcommand 'board'");
  expect(errorLine({ code: 1, stdout: '', stderr: 'boom\n' })).toBe('boom');
  expect(errorLine({ code: 3, stdout: '', stderr: '' })).toBe('paddock exited with code 3');
});
