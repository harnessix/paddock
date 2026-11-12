// The clock of the labels. The interface draws a time as a word ("12 s",
// "4 min", "3 h"), so it only has to draw again when one of those words
// changes: each second for a time under a minute, then each minute, then
// each hour. With no young time on the screen, nothing wakes up.

import { useEffect, useState } from 'react';

/** Seconds until the word of an age changes (model.ts, age). */
export function nextAgeChange(ageSeconds: number): number {
  const s = Math.max(0, Math.floor(ageSeconds));
  if (s < 60) return 1;
  if (s < 3600) return 60 - (s % 60);
  if (s < 86400) return 3600 - (s % 3600);
  return 86400 - (s % 86400);
}

/** Seconds until the word of a time left changes, or Infinity when the time is up. */
export function nextCountdownChange(leftSeconds: number): number {
  const s = Math.floor(leftSeconds);
  if (s <= 0) return Infinity;
  if (s <= 60) return 1;
  if (s <= 3600) return (s % 60) + 1;
  if (s <= 86400) return (s % 3600) + 1;
  return (s % 86400) + 1;
}

export interface Times {
  /** Unix seconds of the past that show as an age. */
  since: number[];
  /** Unix seconds of the future that show as a time left. */
  until: number[];
  /** Unix milliseconds at which something else on the screen changes. */
  at?: number[];
}

/** The milliseconds from `nowMs` until the first word on the screen changes, or Infinity. */
export function msUntilChange(nowMs: number, times: Times): number {
  const now = Math.floor(nowMs / 1000);
  let seconds = Infinity;
  for (const t of times.since) if (t > 0) seconds = Math.min(seconds, nextAgeChange(now - t));
  for (const t of times.until) if (t > 0) seconds = Math.min(seconds, nextCountdownChange(t - now));
  // The words use whole seconds: wake up just after the second begins.
  let ms = Number.isFinite(seconds) ? (now + seconds) * 1000 - nowMs + 20 : Infinity;
  for (const t of times.at ?? []) if (t > nowMs) ms = Math.min(ms, t - nowMs + 20);
  return ms;
}

/** Draw the component again after `delayMs` (never, when it is Infinity). The delay is taken anew after each draw. */
export function useRedrawAfter(delayMs: number): void {
  const [, set] = useState(0);
  useEffect(() => {
    if (!Number.isFinite(delayMs)) return;
    const t = setTimeout(() => set((n) => n + 1), Math.max(50, delayMs));
    return () => clearTimeout(t);
  });
}
