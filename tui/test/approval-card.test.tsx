// The approval card in the shape of the video: a pill "Approval needed"
// with the time left, the question, the command in a dark code box,
// the buttons, and a bar of the time left.
import React from 'react';
import { afterEach, expect, test, vi } from 'vitest';
import { cleanup, render } from 'ink-testing-library';
import { ApprovalCard, PILL, questionOf, timeBar } from '../src/views/approvals.js';
import { makeTheme, ThemeContext } from '../src/theme.js';
import type { Pending } from '../src/types.js';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const NOW = 1_800_000_000;

const CARD: Pending = {
  id: 'r-7-exec',
  sandbox: 'claude-alpha-71c3ac9d',
  harness: 'claude',
  project: '/src/alpha',
  capability: 'exec',
  summary: 'npm publish',
  detail: 'npm publish --access public',
  reason: 'release 0.3.0',
  created: NOW - 60,
  expires: NOW + 180,
  session_allowed: true,
  age_s: 60,
};

function card(p: Pending, width: number, opts: { plain?: boolean; focus?: string } = {}): string {
  vi.spyOn(Date, 'now').mockReturnValue(NOW * 1000);
  const r = render(
    <ThemeContext.Provider value={makeTheme('dark', opts.plain ?? false)}>
      <ApprovalCard pending={p} state={{ focus: opts.focus ?? '3' }} width={width} focused readonly={false} onChoose={() => undefined} onFocusCard={() => undefined} />
    </ThemeContext.Provider>,
  );
  return r.lastFrame() ?? '';
}

test('the approval card', () => {
  expect(card(CARD, 72)).toMatchSnapshot();
});

test('the approval card in plain mode', () => {
  expect(card(CARD, 72, { plain: true })).toMatchSnapshot();
});

test('the parts of the card in their order', () => {
  const lines = card(CARD, 72).split('\n');
  const at = (s: string) => lines.findIndex((l) => l.includes(s));
  expect(at(PILL)).toBe(1);
  expect(lines[1]).toContain('times out in 3 min · asked 1 min ago');
  expect(at('claude wants to run a command:')).toBe(2);
  expect(at('│ exec: npm publish --access public')).toBe(3);
  expect(at('[1] Allow once')).toBeGreaterThan(at('remember: this request only'));
  expect(at('━')).toBeGreaterThan(at('[1] Allow once'));
  // The bar: 3 of the 4 minutes left.
  const bar = (lines[at('━')] as string).replace(/[│ ]+$/, '');
  const full = (bar.match(/━/g) ?? []).length;
  const empty = (bar.match(/─/g) ?? []).length;
  expect(full / (full + empty)).toBeCloseTo(0.75, 1);
  expect(bar).toContain('3 min left');
});

test('a card without a time limit says so and has no bar', () => {
  const f = card({ ...CARD, expires: 0 }, 72);
  expect(f).toContain('no time limit');
  expect(f).not.toContain('━');
  expect(timeBar({ ...CARD, expires: 0 }, 40)).toBe('');
});

test('the bar empties as the time runs out', () => {
  const fixed = { ...CARD, age_s: undefined };
  expect(timeBar(fixed, 30, NOW - 60)).toMatch(/^━+ 4 min left$/);
  expect(timeBar(fixed, 30, NOW + 60)).toMatch(/^━+─+ 2 min left$/);
  expect(timeBar(fixed, 30, NOW + 180)).toMatch(/^─+ 0 s left$/);
});

test('the question names who asks for what', () => {
  expect(questionOf({ id: 'a', harness: 'codex', capability: 'exec' })).toBe('codex wants to run a command:');
  expect(questionOf({ id: 'a', sandbox: 'app-mac-1', capability: 'connect' })).toBe('app-mac-1 wants to connect to a host:');
  expect(questionOf({ id: 'a', harness: 'claude', capability: 'ask' })).toBe('claude asks you:');
  expect(questionOf({ id: 'a', harness: 'grok', capability: 'net' })).toBe('grok asks for net:');
});
