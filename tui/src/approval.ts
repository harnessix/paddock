// The choices of an approval card, and what each one sends to
// `paddock approvals answer`. paddock decides what an answer means. This
// file only names the answers that paddock offers for the request.

import { cleanText, oneLine } from './clean.js';
import type { Pending } from './types.js';

export interface Choice {
  /** The number key, also shown on the button. */
  key: string;
  label: string;
  /** The arguments after `paddock approvals answer ID`. */
  args: string[];
  /** A text answer, sent through stdin. */
  text?: string;
  /** What the answer covers, shown before it is given. */
  scope: string;
  /** A wide or lasting grant: it needs a second confirm. */
  wide: boolean;
  deny: boolean;
}

export const ASK = 'ask';
export const CONNECT = 'connect';

export function choicesOf(p: Pending): Choice[] {
  const cap = p.capability ?? '';
  if (cap === CONNECT) {
    const host = oneLine(p.connect?.host ?? 'the host');
    return [
      { key: '1', label: 'Allow once', args: ['--allow-once'], scope: 'this connection only', wide: false, deny: false },
      { key: '2', label: 'Allow for this session', args: ['--allow-session'], scope: 'this app, host and port until the session ends', wide: false, deny: false },
      { key: '3', label: 'Deny', args: ['--deny'], scope: 'this connection only', wide: false, deny: true },
      { key: '4', label: 'Allow site always', args: ['--allow-site'], scope: `${host} and its subdomains, from now on`, wide: true, deny: false },
      { key: '5', label: 'Allow always', args: ['--allow-always'], scope: 'this app, host and port from now on (a rule)', wide: true, deny: false },
      { key: '6', label: 'Allow every host 10 min', args: ['--allow-everything'], scope: 'all hosts for ten minutes', wide: true, deny: false },
      { key: '7', label: 'Deny always', args: ['--deny-always'], scope: 'this app, host and port from now on (a rule)', wide: true, deny: true },
    ];
  }
  const answers = (p.choices ?? []).map(oneLine).filter((c) => c.length > 0).slice(0, 8);
  if (cap === ASK || answers.length > 0) {
    const list: Choice[] = answers.map((c, i) => ({
      key: String(i + 1),
      label: c,
      args: [],
      text: c,
      scope: 'the agent gets this answer',
      wide: false,
      deny: false,
    }));
    list.push({
      key: String(answers.length + 1),
      label: cap === ASK ? 'Dismiss' : 'Deny',
      args: ['deny'],
      scope: cap === ASK ? 'the agent gets no answer' : 'this request only',
      wide: false,
      deny: true,
    });
    return list;
  }
  const list: Choice[] = [
    { key: '1', label: 'Allow once', args: ['once'], scope: 'this request only', wide: false, deny: false },
  ];
  if (p.session_allowed) {
    list.push({ key: '2', label: 'Allow for this run', args: ['session'], scope: 'this run only: the same request again until it ends', wide: false, deny: false });
  }
  list.push({ key: '3', label: 'Deny', args: ['deny'], scope: 'this request only', wide: false, deny: true });
  if (p.session_allowed) {
    list.push({ key: '4', label: 'Deny for this run', args: ['deny-session'], scope: 'this run only: the same request is denied without a card', wide: false, deny: true });
  }
  return list;
}

/** Whether the request takes a typed answer (a question to you). */
export function takesText(p: Pending): boolean {
  return (p.capability ?? '') === ASK;
}

/** The choice that a letter key picks: y allow once, s for the run, n deny. */
export function choiceForKey(choices: Choice[], input: string): Choice | undefined {
  const byNumber = choices.find((c) => c.key === input);
  if (byNumber) return byNumber;
  if (input === 'y') return choices.find((c) => !c.deny && c.key === '1' && c.args.length > 0);
  if (input === 's') return choices.find((c) => c.args[0] === 'session' || c.args[0] === '--allow-session');
  if (input === 'n') return defaultChoice(choices);
  return undefined;
}

/** The focus a card starts with: Deny. */
export function defaultChoice(choices: Choice[]): Choice | undefined {
  return choices.find((c) => c.deny && !c.wide);
}

/** The request text as paddock sent it, cleaned, as lines. */
export function requestLines(p: Pending): string[] {
  const text = cleanText(p.detail && p.detail.length > 0 ? p.detail : (p.summary ?? ''));
  const lines = text.split('\n').map((l) => l.trimEnd()).filter((l) => l.length > 0);
  if (lines.length === 0) lines.push(oneLine(p.capability ?? 'request'));
  return lines;
}

export const CONFIRM_MS = 3000;
/** The second confirm of a wide grant must come at least this late: a double-click or a key repeat does not count. */
export const CONFIRM_MIN_MS = 300;
/** A card that came to the front by itself takes no answer for this long. */
export const SETTLE_MS = 500;
/** Every line of the request starts with this gutter, so a line of the request can never look like the buttons. */
export const GUTTER = '│ ';
