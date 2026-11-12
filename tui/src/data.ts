// The state of paddock, read through its JSON commands. Each read has its
// own pace: ps and approvals every round (2 s), board and mail every fifth
// round (10 s), because they scan every project and every mailbox. A read
// that fails waits longer each time, a read whose answer does not change
// slows down, and a command that this paddock lacks is not run again. A
// round never starts while the last one runs.

import { useCallback, useEffect, useRef, useState } from 'react';
import { UnsupportedError, type PaddockApi } from './paddock.js';
import type { Approvals, Board, MailBox, Ps } from './types.js';

export interface Snapshot {
  ps?: Ps;
  board?: Board;
  approvals?: Approvals;
  mail?: MailBox[];
  /** One line per command that failed in its last read. */
  errors: string[];
  loaded: boolean;
  at: number;
}

/** board and mail are read every SLOW_FACTOR rounds. */
export const SLOW_FACTOR = 5;
/** At most this many times the base delay after errors. */
export const MAX_BACKOFF = 30;
/**
 * Approvals back off to at most this delay after errors: after a few
 * locked reads in a row, a new card must still show within seconds.
 */
export const APPROVALS_MAX_DELAY_MS = 5000;
/** At most this many times the base delay while nothing changes. */
export const MAX_QUIET = 4;

/**
 * The delay before the next read of a source: doubled per failure in a
 * row (up to MAX_BACKOFF times the base, and never more than `maxFailMs`
 * if it is above the base), and doubled every third round without a
 * change (up to MAX_QUIET times) when `quiet` slows it down.
 */
export function nextDelay(base: number, fails: number, same: number, quiet = true, maxFailMs = Infinity): number {
  if (fails > 0) return Math.min(base * Math.min(MAX_BACKOFF, 2 ** fails), Math.max(base, maxFailMs));
  if (!quiet) return base;
  return base * Math.min(MAX_QUIET, 2 ** Math.floor(same / 3));
}

type Key = 'ps' | 'board' | 'approvals' | 'mail';

interface Source {
  key: Key;
  what: string;
  base: number;
  /** Slow down while the answer does not change. Approvals never do: a new card must show at once. */
  quiet: boolean;
  /** The longest delay after errors, in ms. */
  maxFailMs: number;
  read: () => Promise<unknown>;
  due: number;
  fails: number;
  same: number;
  last: string;
  /** This paddock lacks the command. */
  off: boolean;
}

export function useSnapshot(api: PaddockApi, intervalMs: number): [Snapshot, () => void] {
  const [snap, setSnap] = useState<Snapshot>({ errors: [], loaded: false, at: 0 });
  const busy = useRef(false);
  const loaded = useRef(false);
  const again = useRef(false);
  const alive = useRef(true);
  const errors = useRef(new Map<Key, string>());
  const sources = useRef<Source[]>([]);

  useEffect(() => {
    const mk = (key: Key, what: string, factor: number, quiet: boolean, read: () => Promise<unknown>, maxFailMs = Infinity): Source => ({
      key,
      what,
      base: intervalMs * factor,
      quiet,
      maxFailMs,
      read,
      due: 0,
      fails: 0,
      same: 0,
      last: '',
      off: false,
    });
    sources.current = [
      mk('ps', 'paddock ps', 1, true, () => api.ps()),
      mk('approvals', 'paddock approvals', 1, false, () => api.approvals(), APPROVALS_MAX_DELAY_MS),
      mk('board', 'paddock board', SLOW_FACTOR, true, () => api.board()),
      mk('mail', 'paddock mail', SLOW_FACTOR, true, () => api.mailBoxes()),
    ];
    errors.current = new Map();
  }, [api, intervalMs]);

  const round = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    do {
      again.current = false;
      const now = Date.now();
      const due = sources.current.filter((s) => !s.off && s.due <= now);
      if (due.length === 0) break;
      const results = await Promise.allSettled(due.map((s) => s.read()));
      if (!alive.current) break;
      const values: Partial<Record<Key, unknown>> = {};
      const done = Date.now();
      const errorsBefore = [...errors.current.values()].join('\n');
      let changed = !loaded.current;
      due.forEach((s, i) => {
        const r = results[i] as PromiseSettledResult<unknown>;
        if (r.status === 'fulfilled') {
          const text = JSON.stringify(r.value);
          if (text !== s.last) {
            changed = true;
            values[s.key] = r.value;
          }
          s.same = text === s.last ? s.same + 1 : 0;
          s.last = text;
          s.fails = 0;
          errors.current.delete(s.key);
        } else {
          const e = r.reason;
          if (e instanceof UnsupportedError) s.off = true;
          s.fails += 1;
          errors.current.set(s.key, `${s.what}: ${e instanceof Error ? e.message : String(e)}`);
        }
        s.due = done + nextDelay(s.base, s.fails, s.same, s.quiet, s.maxFailMs);
      });
      if ([...errors.current.values()].join('\n') !== errorsBefore) changed = true;
      // The same answers as before: nothing to draw, so no render at all.
      if (!changed) continue;
      loaded.current = true;
      setSnap((old) => ({
        ps: (values.ps as Ps | undefined) ?? old.ps,
        board: (values.board as Board | undefined) ?? old.board,
        approvals: (values.approvals as Approvals | undefined) ?? old.approvals,
        mail: (values.mail as MailBox[] | undefined) ?? old.mail,
        errors: [...errors.current.values()],
        loaded: true,
        at: Date.now(),
      }));
    } while (again.current && alive.current);
    busy.current = false;
  }, []);

  /** Read everything now: after an action, or when the user asks. */
  const refresh = useCallback(() => {
    for (const s of sources.current) {
      s.due = 0;
      s.same = 0;
    }
    if (busy.current) again.current = true;
    else void round();
  }, [round]);

  useEffect(() => {
    alive.current = true;
    void round();
    // A tick only starts the reads that are due. It runs no process itself.
    const t = setInterval(() => void round(), Math.max(50, Math.floor(intervalMs / 2)));
    return () => {
      alive.current = false;
      clearInterval(t);
    };
  }, [round, intervalMs]);

  return [snap, refresh];
}

/**
 * Poll one read while a view shows it, at the same pace rules: slower
 * after errors and while the answer stays the same. A command that this
 * paddock lacks (UnsupportedError) is not run again. Returns the stop
 * function for an effect.
 */
export function poll(read: () => Promise<unknown>, base: number): () => void {
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let fails = 0;
  let same = 0;
  let last = '';
  const step = async () => {
    try {
      const v = await read();
      const text = JSON.stringify(v);
      same = text === last ? same + 1 : 0;
      last = text;
      fails = 0;
    } catch (e) {
      if (e instanceof UnsupportedError) return;
      fails += 1;
    }
    if (alive) timer = setTimeout(() => void step(), nextDelay(base, fails, same));
  };
  void step();
  return () => {
    alive = false;
    if (timer) clearTimeout(timer);
  };
}
