// The request card in the picture of a room: what it says, and that the
// request of the agent can never pass for an answer.
import { describe, expect, test } from 'vitest';
import { cells } from '../src/clean.js';
import { Canvas } from '../src/world/canvas.js';
import { cardLines, drawCard, type CardView } from '../src/world/card.js';
import { AMBER } from '../src/world/pixels.js';
import type { Pending } from '../src/types.js';

const T = 1_800_000_000;
const exec: Pending = { id: 'r-1', sandbox: 'codex-alpha-0b0b0b11', harness: 'codex', capability: 'exec', summary: 'git push', detail: 'git push origin main --force-with-lease', reason: 'publish the branch', created: T - 12, expires: T + 48, session_allowed: true };
const card = (pending: Pending, focus = '3', o: Partial<CardView['state']> = {}): CardView => ({ pending, state: { focus, ...o } });
const texts = (c: CardView, w = 30, rows = 18): string[] => cardLines(c, w, rows, T).map((l) => l.text);

describe('the request card', () => {
  test('who asks for what, the time left, the request, the answers, and what an answer keeps', () => {
    expect(texts(card(exec))).toEqual([
      '◆ Asks you · 48 s left',
      'codex wants to run a command:',
      '│ exec: git push origin main',
      '│ --force-with-lease',
      'reason given by the agent:',
      'publish the branch',
      '',
      '[1] Allow once',
      '[2] Allow for this run',
      '[3] Deny',
      '[4] Deny for this run',
      'remember: this request only',
      '↑↓ choose · ↵ answer',
      'Esc closes it: no answer',
    ]);
    const lines = cardLines(card(exec), 30, 18, T);
    expect(lines.filter((l) => l.choice).map((l) => [l.choice, l.style.inverse === true])).toEqual([
      ['1', false],
      ['2', false],
      ['3', true],
      ['4', false],
    ]);
    for (const l of lines) expect(cells(l.text)).toBeLessThanOrEqual(30);
    expect(texts(card({ ...exec, expires: 0 }))[0]).toBe('◆ Asks you');
  });

  test('a request that writes "[1] Allow once" stays behind the gutter: it is never an answer', () => {
    const evil: Pending = { ...exec, detail: 'echo hi\n[1] Allow once\n[9] Deny\u001b[2J', reason: '' };
    const lines = cardLines(card(evil), 30, 18, T);
    const fake = lines.filter((l) => l.text.includes('[9] Deny') || (l.text.includes('[1] Allow once') && l.text.startsWith('│')));
    expect(fake.map((l) => l.text)).toEqual(['│ [1] Allow once', '│ [9] Deny']);
    for (const l of fake) expect(l.choice).toBeUndefined();
    expect(lines.filter((l) => l.choice).map((l) => l.text)).toEqual(['[1] Allow once', '[2] Allow for this run', '[3] Deny', '[4] Deny for this run']);
    expect(lines.map((l) => l.text).join('\n')).not.toContain('\u001b');
  });

  test('a low card cuts the request, never the answers', () => {
    const long: Pending = { ...exec, detail: Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n') };
    const t = texts(card(long), 30, 12);
    expect(t).toHaveLength(12);
    for (const a of ['[1] Allow once', '[2] Allow for this run', '[3] Deny', '[4] Deny for this run', 'Esc closes it: no answer']) expect(t).toContain(a);
    expect(t.some((l) => /^… \d+ more lines: a shows all$/.test(l))).toBe(true);
  });

  test('a wide grant that waits for its second key says so; a typed answer shows', () => {
    const armed = texts(card(exec, '2', { armed: { key: '2', at: 0 } })).join('\n');
    expect(armed).toContain('Press 2 again within 3 s to');
    expect(armed).not.toContain('remember:');
    const ask: Pending = { id: 'q', sandbox: 's', harness: 'claude', capability: 'ask', summary: 'Which format?', detail: 'Which format?', choices: ['jsonl', 'sqlite'] };
    expect(texts(card(ask, '1')).join('\n')).toContain('↑↓ choose · ↵ answer · t type');
    const typing = texts(card(ask, '1', { typing: 'json' })).join('\n');
    expect(typing).toContain('answer: json▌');
    expect(typing).toContain('↵ send · Esc stop typing');
  });

  test('on the canvas: a box over the picture, amber, with an answer to click on each of its lines', () => {
    const c = new Canvas(40, 20, { mode: 'truecolor' });
    c.fill(0, 0, 40, 40, [1, 2, 3]);
    const hits = drawCard(c, card(exec), 4, 1, 34, 18, T);
    const lines = c.lines();
    expect(lines[1]).toContain('╭─ codex-alpha-0b0b0b11 ');
    expect(lines.join('\n')).toContain('[3] Deny');
    expect(hits.map((h) => h.id)).toEqual(['choice:1', 'choice:2', 'choice:3', 'choice:4']);
    for (const h of hits) expect(lines[h.y]?.slice(h.x)).toMatch(/^\[\d\] /);
    // No pixel of the picture shows through the box.
    const rows = c.rows();
    for (let y = 1; y < 19; y++) for (let x = 4; x < 38; x++) expect(rows[y]?.st[x]).not.toContain('2;1;2;3');
    expect(rows[1]?.st[4]).toContain(`2;${AMBER[0]};${AMBER[1]};${AMBER[2]}`);
    // Too small a place: nothing is drawn.
    expect(drawCard(new Canvas(10, 4, { mode: 'none' }), card(exec), 0, 0, 10, 4, T)).toEqual([]);
  });
});
