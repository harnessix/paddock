// The clock of the labels: the interface draws again only when a word of
// time on the screen changes.
import { expect, test } from 'vitest';
import { msUntilChange, nextAgeChange, nextCountdownChange } from '../src/clock.js';
import { age } from '../src/model.js';

test('an age changes its word each second, then each minute, hour and day', () => {
  expect(nextAgeChange(0)).toBe(1);
  expect(nextAgeChange(59)).toBe(1);
  expect(nextAgeChange(60)).toBe(60);
  expect(nextAgeChange(119)).toBe(1);
  expect(nextAgeChange(3599)).toBe(1);
  expect(nextAgeChange(3600)).toBe(3600);
  expect(nextAgeChange(5000)).toBe(2200);
  expect(nextAgeChange(86400 + 10)).toBe(86390);
  // The word stays the same until then, and is another one after.
  for (const s of [0, 7, 59, 60, 61, 119, 600, 3599, 3600, 5000, 86399, 86400, 200000]) {
    const n = nextAgeChange(s);
    expect(age(s + n - 1), String(s)).toBe(age(s));
    expect(age(s + n), String(s)).not.toBe(age(s));
  }
});

test('a time left changes its word the same way, down to zero', () => {
  for (const s of [1, 30, 60, 61, 119, 120, 185, 3600, 3601, 7300, 86400, 90000]) {
    const n = nextCountdownChange(s);
    expect(age(s - n + 1), String(s)).toBe(age(s));
    expect(age(s - n), String(s)).not.toBe(age(s));
  }
  expect(nextCountdownChange(0)).toBe(Infinity);
  expect(nextCountdownChange(-5)).toBe(Infinity);
});

test('the wait is until the first word changes, just after its second begins', () => {
  const now = 1_800_000_000_400;
  const s = now / 1000;
  // Nothing on the screen has a time: no wake-up at all.
  expect(msUntilChange(now, { since: [], until: [] })).toBe(Infinity);
  expect(msUntilChange(now, { since: [0], until: [0] })).toBe(Infinity);
  // A run of 10 s: the next second, 600 ms away.
  expect(msUntilChange(now, { since: [Math.floor(s) - 10], until: [] })).toBe(620);
  // Only old runs: the next minute or hour, not the next second.
  expect(msUntilChange(now, { since: [Math.floor(s) - 3700, Math.floor(s) - 100], until: [] })).toBe(20_000 - 400 + 20);
  expect(msUntilChange(now, { since: [Math.floor(s) - 3700], until: [] })).toBe(3_500_000 - 400 + 20);
  // A card that times out in 185 s shows "3 min" for 6 s more.
  expect(msUntilChange(now, { since: [], until: [Math.floor(s) + 185] })).toBe(6000 - 400 + 20);
  expect(msUntilChange(now, { since: [Math.floor(s) - 3700], until: [], at: [now + 5000] })).toBe(5020);
});
