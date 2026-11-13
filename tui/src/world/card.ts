// The request card of a room, on the canvas beside the door: who asks for
// what, the request as the agent wrote it, and the answers. The answers
// and their safety rules (a wide grant needs its key twice, a card that
// came by itself takes no answer at once) are those of the Approvals tab:
// this file only draws.

import { choicesOf, GUTTER, requestLines, takesText, type Choice } from '../approval.js';
import { cells, cut, oneLine } from '../clean.js';
import { t } from '../i18n/index.js';
import { age } from '../model.js';
import type { Pending } from '../types.js';
import { questionOf, timeLeft, type CardState } from '../views/approvals.js';
import type { Canvas, TextStyle } from './canvas.js';
import { wrap } from './inspect.js';
import * as px from './pixels.js';
import type { Hit } from './scene.js';

export interface CardView {
  pending: Pending;
  state: CardState;
}

interface Line {
  text: string;
  style: TextStyle;
  /** The key of the answer on this line. */
  choice?: string;
}

/** The lines of the card for a width and a number of rows. The request is cut first, never the answers. */
export function cardLines(card: CardView, width: number, rows: number, now: number, art = false, light = false): Line[] {
  const p = card.pending;
  const choices = choicesOf(p);
  const amber: TextStyle = art ? { fg: px.amberText(light), bold: true } : { bold: true };
  const tl = timeLeft(p, now);
  const head: Line[] = [{ text: cut(tl ? t('world.card.asksLeft', { left: age(tl.left) }) : t('world.card.asks'), width), style: amber }];
  for (const l of wrap(questionOf(p), width)) head.push({ text: l, style: {} });

  const tail: Line[] = [{ text: '', style: {} }];
  for (const c of choices) {
    const focused = c.key === card.state.focus;
    tail.push({ text: cut(`[${c.key}] ${c.label}${c.wide ? ' ⚠' : ''}`, width), style: { inverse: focused, bold: focused }, choice: c.key });
  }
  const focus: Choice | undefined = choices.find((c) => c.key === card.state.focus);
  const armed = card.state.armed ? choices.find((c) => c.key === card.state.armed?.key) : undefined;
  if (armed) for (const l of wrap(t('world.card.confirm', { key: armed.key, scope: armed.scope }), width)) tail.push({ text: l, style: amber });
  else if (focus) for (const l of wrap(t('world.card.remember', { scope: focus.scope }), width)) tail.push({ text: l, style: { dim: true } });
  if (card.state.typing !== undefined) {
    tail.push({ text: cut(t('world.card.typing', { text: card.state.typing }), width), style: {} });
    tail.push({ text: cut(t('world.card.typingKeys'), width), style: { dim: true } });
  } else {
    tail.push({ text: cut(takesText(p) ? t('world.card.keysType') : t('world.card.keys'), width), style: { dim: true } });
    tail.push({ text: cut(t('world.card.close'), width), style: { dim: true } });
  }

  // The request, each line behind the gutter: a line such as "[1] Deny" cannot pass for an answer.
  const req: Line[] = [];
  const inner = Math.max(1, width - cells(GUTTER));
  requestLines(p).forEach((l, i) => {
    for (const part of wrap(`${i === 0 ? `${oneLine(p.capability ?? '?')}: ` : ''}${l}`, inner)) req.push({ text: GUTTER + part, style: {} });
  });
  if (p.reason) for (const l of wrap(t('world.card.reason', { reason: oneLine(p.reason) }), width)) req.push({ text: l, style: { dim: true } });
  const room = Math.max(1, rows - head.length - tail.length);
  const shown = req.length > room ? [...req.slice(0, room - 1), { text: cut(t('world.card.more', { n: req.length - room + 1 }), width), style: { dim: true } as TextStyle }] : req;
  return [...head, ...shown, ...tail].slice(0, rows);
}

/** The card as a box on the canvas, over what is there. Returns the answers as things to click. */
export function drawCard(c: Canvas, card: CardView, x: number, y: number, w: number, h: number, now: number): Hit[] {
  const hits: Hit[] = [];
  if (w < 16 || h < 6) return hits;
  c.fill(x, y * 2, w, h * 2, null);
  c.clearText(x, y, w, h);
  const line: TextStyle = c.art ? { fg: px.amberText(c.light) } : { bold: true };
  c.text(x, y, '╭' + '─'.repeat(w - 2) + '╮', line);
  c.text(x + 2, y, cut(` ${oneLine(card.pending.sandbox || card.pending.harness || t('world.card.request'))} `, w - 4), { ...line, bold: true });
  for (let r = 1; r < h - 1; r++) {
    c.text(x, y + r, '│', line);
    c.text(x + 1, y + r, ' '.repeat(w - 2), {});
    c.text(x + w - 1, y + r, '│', line);
  }
  c.text(x, y + h - 1, '╰' + '─'.repeat(w - 2) + '╯', line);
  cardLines(card, w - 4, h - 2, now, c.art, c.light).forEach((l, i) => {
    c.text(x + 2, y + 1 + i, l.choice ? l.text + ' '.repeat(Math.max(0, w - 4 - cells(l.text))) : l.text, l.style);
    if (l.choice) hits.push({ id: `choice:${l.choice}`, kind: 'door', x: x + 2, y: y + 1 + i, w: w - 4, h: 1 });
  });
  return hits;
}
