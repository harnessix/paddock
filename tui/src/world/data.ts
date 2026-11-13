// What the world shows besides the runs, the board, the approvals and the
// mail: the tools, the shared notes, the changed files, the use of the
// accounts, the saved sessions. Each is one JSON command of paddock, read
// only while the world is on the screen, at the slow pace of the board.
// A command that this paddock lacks is not run again, and its part of
// the world stays out.

import { useEffect, useState } from 'react';
import { poll, SLOW_FACTOR } from '../data.js';
import type { PaddockApi } from '../paddock.js';
import type { WorldExtra } from './model.js';

export function useWorldExtra(api: PaddockApi, active: boolean, intervalMs: number): WorldExtra {
  const [extra, setExtra] = useState<WorldExtra>({});
  useEffect(() => {
    if (!active) return;
    let alive = true;
    const last = new Map<keyof WorldExtra, string>();
    const source = <K extends keyof WorldExtra>(key: K, read: () => Promise<WorldExtra[K]>): (() => void) =>
      poll(
        () =>
          read().then((v) => {
            // The same answer as before: nothing to draw, so no render at all.
            const text = JSON.stringify(v);
            if (alive && last.get(key) !== text) {
              last.set(key, text);
              setExtra((old) => ({ ...old, [key]: v }));
            }
            return v;
          }),
        intervalMs * SLOW_FACTOR,
      );
    const stops = [
      source('devices', () => api.devices()),
      source('kv', () => api.kvOverview()),
      source('files', () => api.files()),
      source('forage', () => api.forage()),
      source('horses', () => api.horses()),
    ];
    return () => {
      alive = false;
      for (const s of stops) s();
    };
  }, [api, active, intervalMs]);
  return extra;
}
