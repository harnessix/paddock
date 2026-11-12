// The keys, one by one. Ink hands over what the terminal sent in one read.
// With fast typing, a key that repeats or a slow connection, one read holds
// several keys: "jjj", or "/beta" and Enter. Each key must count, in its
// order, and each must see what the key before it did. So the keys wait in
// a line, and the interface takes the next one after it drew the last.
//
// A paste is not typing. The terminal marks a paste (bracketed paste), and
// it goes into the open text field or nowhere: its letters never act as
// commands.

import { useEffect, useRef, useState } from 'react';
import { useInput, usePaste, type Key } from 'ink';

export interface KeyEvent {
  input: string;
  key: Key;
  /** A run of printable characters: typed text, or several command keys. */
  text?: boolean;
  /** Pasted text. */
  paste?: boolean;
  /** Command keys already taken from this run. */
  taken?: number;
}

/** At most this many command keys from one read: more is a paste from a terminal that does not mark it. */
export const TYPE_AHEAD_MAX = 4;
/** A read longer than this is a paste: its line breaks are not Enter. */
export const PASTE_MIN = 64;

const FLAGS = ['upArrow', 'downArrow', 'leftArrow', 'rightArrow', 'pageDown', 'pageUp', 'home', 'end', 'return', 'escape', 'ctrl', 'tab', 'backspace', 'delete', 'meta'] as const;

function special(key: Key): boolean {
  return FLAGS.some((f) => key[f]);
}

/** An escape sequence that Ink does not know, without its ESC: never keys. */
function isSequence(input: string): boolean {
  return /^\[[\d;?<>=]*[ -/]*[@-~]$/.test(input) || /^O[A-Za-z]$/.test(input);
}

/** The keys in one input of Ink. A run of printable characters stays one piece: who takes it decides. */
export function splitInput(input: string, key: Key): KeyEvent[] {
  if (special(key) || Array.from(input).length <= 1 || isSequence(input)) return [{ input, key }];
  const base: Key = { ...key, shift: false };
  const out: KeyEvent[] = [];
  const paste = input.length > PASTE_MIN;
  let run = '';
  const flush = (): void => {
    if (run) out.push({ input: run, key: base, text: true });
    run = '';
  };
  for (const ch of input) {
    const cp = ch.codePointAt(0) ?? 0;
    if (cp >= 0x20 && cp !== 0x7f) {
      run += ch;
      continue;
    }
    // In raw mode Enter is CR. A line feed comes only from pasted text.
    if (cp === 0x0a || (paste && cp === 0x0d)) {
      run += ' ';
      continue;
    }
    flush();
    if (cp === 0x0d) out.push({ input: '\r', key: { ...base, return: true } });
    else if (cp === 0x09) out.push({ input: '\t', key: { ...base, tab: true } });
    else if (cp === 0x7f || cp === 0x08) out.push({ input: '', key: { ...base, backspace: true } });
    else if (cp === 0x1b) out.push({ input: '', key: { ...base, escape: true } });
    else if (cp >= 0x01 && cp <= 0x1a) out.push({ input: String.fromCharCode(0x60 + cp), key: { ...base, ctrl: true } });
  }
  flush();
  return out;
}

/**
 * The next key of the line. In a text field a run of characters is one
 * piece of text. Elsewhere each character is a command key, and after
 * TYPE_AHEAD_MAX of them the rest of the run is dropped.
 * Returns null when a piece was dropped.
 */
export function takeKey(queue: KeyEvent[], textMode: boolean): KeyEvent | null {
  const e = queue[0];
  if (!e) return null;
  if (e.paste) {
    queue.shift();
    return textMode ? e : null;
  }
  if (!e.text || textMode) {
    queue.shift();
    return e;
  }
  const chars = Array.from(e.input);
  const taken = e.taken ?? 0;
  if (taken >= TYPE_AHEAD_MAX) {
    queue.shift();
    return null;
  }
  const first = chars[0] as string;
  const rest = chars.slice(1).join('');
  if (rest) queue[0] = { ...e, input: rest, taken: taken + 1 };
  else queue.shift();
  return { input: first, key: { ...e.key, shift: first !== first.toLowerCase() && first === first.toUpperCase() } };
}

export interface Keys {
  /** Drop the keys that wait: a question that can destroy something must not get its answer from keys typed before it showed. */
  clear: () => void;
}

/**
 * Hand each key to `onKey`, one per draw of the interface. `report` gets
 * every input first and returns true for what is not a key (a mouse or a
 * focus report).
 */
export function useKeys(opts: {
  onKey: (input: string, key: Key) => void;
  /** A text field is open: runs of characters are text. */
  textMode: boolean;
  report: (input: string) => boolean;
  /** Pasted text was dropped, because no text field is open. */
  onDrop?: () => void;
}): Keys {
  const queue = useRef<KeyEvent[]>([]);
  const [pump, setPump] = useState(0);
  // The draw that this closure belongs to, and the draw in which a key was last handled at once.
  const draws = useRef(0);
  const handledIn = useRef(-1);
  draws.current++;
  // The handler and the mode of the newest draw: Ink may still call the handler of an older one.
  const latest = useRef(opts);
  latest.current = opts;

  const next = (): void => {
    const o = latest.current;
    const hadPaste = queue.current[0]?.paste === true;
    const e = takeKey(queue.current, o.textMode);
    if (e) o.onKey(e.input, e.key);
    else if (hadPaste) o.onDrop?.();
  };

  useEffect(() => {
    if (queue.current.length === 0) return;
    next();
    if (queue.current.length > 0) setPump((n) => n + 1);
    // One key per draw: the next one sees the state that this one made.
  }, [pump]);

  const push = (events: KeyEvent[]): void => {
    const first = events[0];
    // The usual case, one key and nothing before it: handle it now, in this draw.
    const o = latest.current;
    const single = events.length === 1 && first !== undefined && !first.paste && !(first.text && Array.from(first.input).length > 1 && !o.textMode);
    if (single && queue.current.length === 0 && handledIn.current !== draws.current) {
      handledIn.current = draws.current;
      o.onKey(first.input, first.key);
      return;
    }
    queue.current.push(...events);
    setPump((n) => n + 1);
  };

  useInput((input, key) => {
    if (latest.current.report(input)) return;
    push(splitInput(input, key));
  });
  usePaste((text) => push([{ input: text, key: BLANK, text: true, paste: true }]));

  return {
    clear: () => {
      queue.current.length = 0;
    },
  };
}

const BLANK: Key = {
  upArrow: false,
  downArrow: false,
  leftArrow: false,
  rightArrow: false,
  pageDown: false,
  pageUp: false,
  home: false,
  end: false,
  return: false,
  escape: false,
  ctrl: false,
  shift: false,
  tab: false,
  backspace: false,
  delete: false,
  meta: false,
  super: false,
  hyper: false,
  capsLock: false,
  numLock: false,
};
