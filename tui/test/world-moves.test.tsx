// Every motion of the design (WORLD1), each with a clock that the test
// moves by hand: what starts it, its frames, and the end state. Then
// Reduce Motion, and that nothing works while nothing moves.
//
// The key frames are also files of test/__screens__ (text with the colour
// codes of a terminal): `cat test/__screens__/motion-climb-2.ans`.
import React from 'react';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { cleanup, render } from 'ink-testing-library';
import { detectArt } from '../src/art.js';
import { ArtContext, ArtLayer, FPS, SceneView } from '../src/art-layer.js';
import { desktopReducedMotion, fullMotion, wantsReducedMotion, type ReadCommand } from '../src/motion-pref.js';
import type { Row } from '../src/screen.js';
import { rowText } from '../src/world/canvas.js';
import { crossfade } from '../src/world/fade.js';
import { arc, FAN_LOOP, FRAMES, LOFT_VISIT, Motion, REDUCED, spring } from '../src/world/motion.js';
import { deviceCall, loftWrite, observe, writer } from '../src/world/observe.js';
import { fanAt } from '../src/world/ranch.js';
import { drawWorld, planRoom, type View } from '../src/world/scene.js';
import { worldScene } from '../src/views/world.js';
import type { World } from '../src/world/model.js';
import { CLAUDE, CODEX, demoWorld, GEMINI, GROK, T } from './world-demo.js';
import { fakeWorld, renderApp, type Rendered } from './helpers.js';

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

const SCREENS = join(dirname(fileURLToPath(import.meta.url)), '__screens__');
const ALPHA = '/src/alpha';
const W = 100;
const H = 31;
const room = (id: string, plumbing = false): View => ({ level: 'room', house: ALPHA, room: id, sel: '', plumbing });
const ranch: View = { level: 'ranch', house: '', room: '', sel: '', plumbing: false };
const building: View = { level: 'building', house: ALPHA, room: '', sel: '', plumbing: false };
const loft: View = { level: 'loft', house: ALPHA, room: '', sel: '', plumbing: false };

function rows(world: World, view: View, motion: Motion, tick: number, mode: 'truecolor' | '256' | 'none' = 'truecolor'): Row[] {
  return drawWorld(world, { w: W, h: H, mode, tick, motion, view, t: T }).canvas.rows();
}

const text = (r: readonly Row[]): string => r.map(rowText).join('\n');

/** The picture as a terminal gets it. */
function pic(world: World, view: View, motion: Motion, tick: number): string {
  return text(rows(world, view, motion, tick));
}

/** The picture at rest: no motion, at the same frame of the loops. */
function rest(world: World, view: View, tick: number, reduce = false): string {
  return pic(world, view, new Motion(reduce), tick);
}

/** A key frame as a file of test/__screens__. */
function save(name: string, picture: string): string {
  mkdirSync(SCREENS, { recursive: true });
  writeFileSync(join(SCREENS, `motion-${name}.ans`), picture + '\n');
  return picture;
}

/** The cells of a rectangle of a picture. */
function region(r: readonly Row[], x0: number, x1: number, y0: number, y1: number): string {
  return r
    .slice(y0, y1)
    .map((row) => row.ch.slice(x0, x1).join('') + row.st.slice(x0, x1).join('|'))
    .join('\n');
}

/** How many cells of a picture have a colour. */
function count(r: readonly Row[], rgb: readonly number[]): number {
  const code = `2;${rgb[0]};${rgb[1]};${rgb[2]}`;
  let n = 0;
  for (const row of r) for (const s of row.st) if (s.includes(code)) n++;
  return n;
}

/** All frames differ from each other. */
function distinct(frames: string[]): boolean {
  return new Set(frames).size === frames.length;
}

const ms = (frames: number): number => (frames * 1000) / FPS;
const WHITE = [255, 255, 255] as const;

describe('the lengths of the design', () => {
  test('each motion is within one frame of its time', () => {
    const design: [keyof typeof FRAMES, number][] = [
      ['ask', 500],
      ['answer', 400],
      ['net', 300],
      ['mail', 200],
      ['hop', 300],
      ['freeze', 600],
      ['thaw', 600],
      ['climb', 600],
      ['descend', 600],
      ['note', 250],
      ['spark', 400],
      ['flip', 200],
      ['join', 500],
      ['spawn', 800],
      ['end', 500],
      ['zoom', 450],
      ['pan', 350],
      ['plumb', 300],
      ['unplumb', 300],
      // Reduce Motion: the cross-fade, the bright cable, the dot.
      ['fade', 150],
      ['glow', 1000],
      ['dot', 2000],
    ];
    for (const [k, want] of design) expect(Math.abs(ms(FRAMES[k]) - want), k).toBeLessThanOrEqual(125);
    // The fan turns once in 2 s.
    expect(ms(FAN_LOOP)).toBe(2000);
    expect(LOFT_VISIT).toBe(FRAMES.climb + FRAMES.write + FRAMES.descend);
  });

  test('the camera is a spring that settles, and a hop goes up and comes down', () => {
    expect(spring(0)).toBe(0);
    expect(spring(1)).toBe(1);
    const s = [0.25, 0.5, 0.75].map(spring);
    // Fast at first, then slow, and never past its end.
    expect(s[0]).toBeGreaterThan(0.4);
    expect((s[1] as number) - (s[0] as number)).toBeGreaterThan((s[2] as number) - (s[1] as number));
    expect(s[2]).toBeLessThan(1);
    expect([0, 0.5, 1].map(arc)).toEqual([0, 1, 0]);
  });

  test('a motion that waits for its frame has not begun', () => {
    const m = new Motion();
    m.start('note', 'h', 10);
    expect(m.progress('note', 'h', 9)).toBe(1);
    expect(m.running('note', 'h', 9)).toBe(false);
    // The clock runs until it comes.
    expect(m.active(9)).toBe(true);
    expect(m.progress('note', 'h', 10)).toBe(0);
    expect(m.active(12)).toBe(false);
  });
});

describe('what starts the new motions', () => {
  test('a question in the mail, a tool that starts, a note of the shared notes, a note in the loft, a new mode of the window', () => {
    const before = demoWorld((d) => {
      d.mail = [{ id: CLAUDE, state: 'running', unread_up: 2, unread_ask: 0 }];
      d.board.projects[0]!.loft_commits = 12;
      // Only claude works in alpha: paddock does not say who writes, so one candidate is the answer.
      d.ps.sandboxes = d.ps.sandboxes.filter((s) => s.id === CLAUDE || s.id === GROK);
      d.ps.sandboxes[0]!.devices = [];
    });
    const after = demoWorld((d) => {
      d.mail = [{ id: CLAUDE, state: 'running', unread_up: 3, unread_ask: 1 }];
      d.board.projects[0]!.loft_commits = 13;
      d.ps.sandboxes = d.ps.sandboxes.filter((s) => s.id === CLAUDE || s.id === GROK);
      d.ps.sandboxes[0]!.network = 'api';
      d.extra.kv!.user = { keys: 13, leases: 2 };
      d.extra.kv!.pastures[0]!.keys = 5;
    });
    const m = new Motion();
    observe(m, before, after, 0);
    expect(m.running('hop', CLAUDE, 0)).toBe(true);
    expect(m.running('mail', CLAUDE, 0)).toBe(true);
    // The instance of `train` runs now and did not before: a call.
    expect(m.running('spark', `${CLAUDE}:train`, 0)).toBe(true);
    expect(m.running('spark', `${CLAUDE}:lint`, 0)).toBe(false);
    expect(m.running('flip', 'gate', 0)).toBe(true);
    expect(m.running('flip', `yard:${ALPHA}:team`, 0)).toBe(true);
    expect(m.running('climb', CLAUDE, 0)).toBe(true);
    expect(m.running('net', CLAUDE, 0)).toBe(true);
    expect(m.from('net', CLAUDE)).toBe('egress');
    // The same world again: nothing.
    const again = new Motion();
    observe(again, after, after, 0);
    expect(again.active(0)).toBe(false);
  });

  test('without the fields that paddock does not report yet, the mailbox does not hop and no one climbs', () => {
    const before = demoWorld();
    const after = demoWorld((d) => {
      d.mail = [{ id: CLAUDE, state: 'running', unread_up: 5 }];
    });
    expect(after.houses[0]?.notes).toBe(null);
    expect(after.houses[0]?.rooms.find((r) => r.id === CLAUDE)?.unreadAsk).toBe(null);
    const m = new Motion();
    observe(m, before, after, 0);
    expect(m.running('mail', CLAUDE, 0)).toBe(true);
    expect(m.running('hop', CLAUDE, 0)).toBe(false);
    expect(m.running('climb', CLAUDE, 0)).toBe(false);
  });

  test('two runs that work: paddock does not say who wrote, so only the notebook moves', () => {
    const w = demoWorld();
    const h = w.houses[0]!;
    expect(writer(h)).toBe(null);
    const m = new Motion();
    expect(loftWrite(m, w, ALPHA, null, 0)).toBe(true);
    expect(m.running('note', ALPHA, 0)).toBe(true);
    expect(m.trip(CLAUDE, 0).where).toBe('room');
    // An event that names the run: that one climbs.
    const k = new Motion();
    loftWrite(k, w, ALPHA, CODEX, 0);
    expect(k.trip(CODEX, 0).where).toBe('ladder');
    expect(loftWrite(k, w, '/src/website', null, 0)).toBe(false);
  });

  test('a call of a tool from an event: the mode of the event, or of the tool', () => {
    const w = demoWorld();
    const m = new Motion();
    expect(deviceCall(m, w, { sandbox: CLAUDE, device: 'screenshot' }, 0)).toBe(true);
    // screenshot asks: after the spark, the card rises.
    expect(m.running('rise', `${CLAUDE}:screenshot`, FRAMES.spark)).toBe(true);
    expect(deviceCall(m, w, { sandbox: CLAUDE, device: 'lint', mode: 'allow' }, 0)).toBe(true);
    expect(m.running('rise', `${CLAUDE}:lint`, FRAMES.spark)).toBe(false);
    expect(deviceCall(m, w, { sandbox: CLAUDE, device: 'no-such-tool' }, 0)).toBe(false);
    expect(deviceCall(m, w, { sandbox: 'no-such-run', device: 'lint' }, 0)).toBe(false);
  });
});

describe('requests: the walk to the door and back', () => {
  test('after the answer the character walks back in 3 frames', () => {
    const asked = demoWorld();
    const answered = demoWorld((d) => {
      d.pending = [];
    });
    const m = new Motion();
    observe(m, asked, answered, 40);
    expect(m.running('answer', CODEX, 42)).toBe(true);
    const frames = [40, 41, 42].map((t) => pic(answered, room(CODEX), m, t));
    // It leaves the door at once. The spring settles: in the last frame it is at its place already.
    frames.slice(0, 2).forEach((f, i) => expect(f, `frame ${i}`).not.toBe(rest(answered, room(CODEX), 40 + i)));
    expect(frames[0]).not.toBe(frames[1]);
    save('walk-back-1', frames[1] as string);
    expect(pic(answered, room(CODEX), m, 43)).toBe(rest(answered, room(CODEX), 43));
    expect(m.active(43)).toBe(false);
    // The walk there, for the file.
    const k = new Motion();
    observe(k, answered, asked, 0);
    save('walk-2', pic(asked, room(CODEX), k, 2));
    save('walk-end', pic(asked, room(CODEX), k, FRAMES.ask));
  });
});

describe('the window', () => {
  const api = demoWorld();
  const open = demoWorld((d) => {
    for (const s of d.ps.sandboxes) if (s.id === CODEX) s.network = 'egress';
  });
  const plan = planRoom(W, H, true);
  /** The cells of the window of the room view. */
  const win = (r: readonly Row[]): string => region(r, plan.x + 31, plan.x + 38, Math.floor((plan.y + 2) / 2), Math.ceil((plan.y + 10) / 2));

  test('the old window turns into the new one in 3 frames', () => {
    const m = new Motion();
    observe(m, api, open, 0);
    const frames = [0, 1, 2, 3].map((t) => win(rows(open, room(CODEX), m, t)));
    // It starts as the old window and ends as the new one. Between them it is neither.
    expect(frames[0]).toBe(win(rows(api, room(CODEX), new Motion(), 0)));
    expect(frames[3]).toBe(win(rows(open, room(CODEX), new Motion(), 3)));
    expect(distinct(frames)).toBe(true);
    save('window-1', pic(open, room(CODEX), m, 1));
    expect(m.active(3)).toBe(false);
  });

  test('in the building and on the ranch too', () => {
    for (const view of [building, ranch]) {
      const m = new Motion();
      observe(m, api, open, 0);
      expect(pic(open, view, m, 1), view.level).not.toBe(rest(open, view, 1));
      expect(pic(open, view, m, 3), view.level).toBe(rest(open, view, 3));
    }
  });

  test('Reduce Motion: an instant swap', () => {
    const m = new Motion(true);
    observe(m, api, open, 0);
    expect(m.active(0)).toBe(false);
    expect(pic(open, room(CODEX), m, 0)).toBe(rest(open, room(CODEX), 0));
  });
});

describe('the mailbox', () => {
  const none = demoWorld((d) => {
    d.mail = [{ id: CLAUDE, state: 'running', unread_up: 0, unread_ask: 0 }];
  });
  const status = demoWorld((d) => {
    d.mail = [{ id: CLAUDE, state: 'running', unread_up: 1, unread_ask: 0 }];
  });
  const ask = demoWorld((d) => {
    d.mail = [{ id: CLAUDE, state: 'running', unread_up: 2, unread_ask: 1 }];
  });
  const plan = planRoom(W, H, true);
  const box = (r: readonly Row[]): string => region(r, Math.max(0, plan.x - 7), plan.x, Math.floor((plan.y + plan.h - 16) / 2), Math.ceil((plan.y + plan.h) / 2));

  test('the flag goes up in 2 frames: down, half, up', () => {
    const m = new Motion();
    observe(m, none, status, 0);
    // Status mail does not make the box hop.
    expect(m.running('hop', CLAUDE, 0)).toBe(false);
    const frames = [0, 1, 2].map((t) => box(rows(status, room(CLAUDE), m, t)));
    expect(frames[0]).toBe(box(rows(none, room(CLAUDE), new Motion(), 0)));
    expect(frames[2]).toBe(box(rows(status, room(CLAUDE), new Motion(), 2)));
    expect(distinct(frames)).toBe(true);
    save('mailbox-flag-1', pic(status, room(CLAUDE), m, 1));
    expect(m.active(2)).toBe(false);
  });

  test('a question: the box hops once, up and down in 3 frames', () => {
    const seen = new Motion();
    observe(seen, status, ask, 0);
    expect(seen.running('hop', CLAUDE, 0)).toBe(true);
    // The hop alone, without the flag that goes up at the same time.
    const m = new Motion();
    m.start('hop', CLAUDE, 0);
    const still = box(rows(ask, room(CLAUDE), new Motion(), 0));
    const frames = [0, 1, 2, 3].map((t) => box(rows(ask, room(CLAUDE), m, t)));
    // One pixel up, two, one, and on the ground again.
    expect(frames[0]).not.toBe(still);
    expect(frames[1]).not.toBe(frames[0]);
    expect(frames[2]).toBe(frames[0]);
    expect(frames[3]).toBe(still);
    save('mailbox-hop-1', pic(ask, room(CLAUDE), m, 1));
    expect(m.active(3)).toBe(false);
    // In the building and on the ranch the flag hops too.
    m.start('hop', CLAUDE, 0);
    expect(pic(ask, ranch, m, 1)).not.toBe(rest(ask, ranch, 1));
    expect(pic(ask, ranch, m, 3)).toBe(rest(ask, ranch, 3));
    // The building on a screen that has the rows for its rooms at their full height.
    const tall = (motion: Motion, tick: number): string => text(drawWorld(ask, { w: 120, h: 45, mode: 'truecolor', tick, motion, view: { ...building, sel: `room:${CLAUDE}` }, t: T }).canvas.rows());
    expect(tall(m, 1)).not.toBe(tall(new Motion(), 1));
    expect(tall(m, 3)).toBe(tall(new Motion(), 3));
  });
});

describe('frost', () => {
  test('Reduce Motion: no growth. The paused room has its tint and a snowflake badge', () => {
    const before = demoWorld((d) => {
      for (const s of d.ps.sandboxes) if (s.id === GROK) delete s.frozen_since;
    });
    const after = demoWorld();
    const m = new Motion(true);
    observe(m, before, after, 0);
    expect(m.active(0)).toBe(false);
    for (const view of [room(GROK), building]) {
      const reduced = rows(after, view, m, 0);
      const full = rows(after, view, new Motion(), 0);
      // The badge has white pixels that the frost does not have.
      expect(count(reduced, WHITE), view.level).toBeGreaterThan(count(full, WHITE));
    }
    expect(pic(after, room(GROK), m, 0)).toBe(pic(after, room(GROK), m, 9));
    save('reduce-paused', pic(after, room(GROK), m, 0));
    // A run that is not paused has no badge.
    expect(pic(after, room(CLAUDE), m, 0)).toBe(rest(after, room(CLAUDE), 0));
  });

  test('the frost grows and melts, for the files', () => {
    const thawed = demoWorld((d) => {
      for (const s of d.ps.sandboxes) if (s.id === GROK) delete s.frozen_since;
    });
    const m = new Motion();
    observe(m, thawed, demoWorld(), 0);
    save('frost-2', pic(demoWorld(), room(GROK), m, 2));
    const k = new Motion();
    observe(k, demoWorld(), thawed, 0);
    save('thaw-2', pic(thawed, room(GROK), k, 2));
    expect(pic(thawed, room(GROK), k, FRAMES.thaw)).toBe(rest(thawed, room(GROK), FRAMES.thaw));
  });
});

describe('the loft: the climb, the note, the way back', () => {
  const w = demoWorld();
  const UP = FRAMES.climb;
  const DOWN = FRAMES.climb + FRAMES.write;

  test('the stops of the trip: 5 frames on the ladder, 8 upstairs, 5 down', () => {
    const m = new Motion();
    m.wrote(ALPHA, CLAUDE, 100);
    const at = (t: number) => m.trip(CLAUDE, 100 + t);
    // The middle of each frame: a step up in the first, near the top in the last.
    expect(at(0).where).toBe('ladder');
    expect(at(0).p).toBeCloseTo(0.1);
    expect(at(UP - 1).where).toBe('ladder');
    expect(at(UP - 1).p).toBeCloseTo(0.9);
    expect(at(UP).where).toBe('loft');
    expect(at(DOWN - 1).where).toBe('loft');
    expect(at(DOWN).where).toBe('ladder');
    expect(at(DOWN).p).toBeCloseTo(0.9);
    expect(at(LOFT_VISIT - 1).where).toBe('ladder');
    expect(at(LOFT_VISIT).where).toBe('room');
    // The notebook slides in while the character still writes: its last 2 frames upstairs.
    expect(m.running('note', ALPHA, 100 + DOWN - FRAMES.note)).toBe(true);
    expect(m.running('note', ALPHA, 100 + DOWN - FRAMES.note - 1)).toBe(false);
    expect(m.active(100 + LOFT_VISIT - 1)).toBe(true);
    expect(m.active(100 + LOFT_VISIT)).toBe(false);
  });

  test('in the room: it climbs out through the ceiling, writes in the attic, and comes back', () => {
    const m = new Motion();
    m.wrote(ALPHA, CLAUDE, 0);
    const frames = Array.from({ length: LOFT_VISIT }, (_, t) => pic(w, room(CLAUDE), m, t));
    frames.forEach((f, t) => expect(f, `frame ${t}`).not.toBe(rest(w, room(CLAUDE), t)));
    // Each frame on the ladder is a new place.
    expect(distinct(frames.slice(0, UP))).toBe(true);
    expect(distinct(frames.slice(DOWN))).toBe(true);
    save('climb-2', frames[2] as string);
    save('climb-write', frames[UP + 1] as string);
    save('note-slide', frames[DOWN - 1] as string);
    save('climb-down-2', frames[DOWN + 2] as string);
    // The notebook: at rest before it slides, on its way for 2 frames.
    const plan = planRoom(W, H, true);
    const shelf = (t: number, motion: Motion): string => region(rows(w, room(CLAUDE), motion, t), plan.x + 10, plan.x + 20, Math.floor((plan.y - 7) / 2), Math.ceil(plan.y / 2));
    expect(shelf(DOWN - 3, m)).toBe(shelf(DOWN - 3, new Motion()));
    expect(shelf(DOWN - 2, m)).not.toBe(shelf(DOWN - 2, new Motion()));
    expect(shelf(DOWN - 1, m)).not.toBe(shelf(DOWN - 2, m));
    expect(shelf(DOWN, m)).toBe(shelf(DOWN, new Motion()));
    // The end state: the room as it was.
    expect(pic(w, room(CLAUDE), m, LOFT_VISIT)).toBe(rest(w, room(CLAUDE), LOFT_VISIT));
  });

  test('in the loft and in the building: the character comes up, writes, and goes', () => {
    for (const view of [loft, { ...building, sel: `room:${CLAUDE}` }]) {
      const m = new Motion();
      m.wrote(ALPHA, CLAUDE, 0);
      for (const t of [1, UP, UP + 3, DOWN + 1]) expect(pic(w, view, m, t), `${view.level} ${t}`).not.toBe(rest(w, view, t));
      // It writes: the loop of its work moves upstairs.
      expect(pic(w, view, m, UP), view.level).not.toBe(pic(w, view, m, UP + 1));
      expect(pic(w, view, m, LOFT_VISIT), view.level).toBe(rest(w, view, LOFT_VISIT));
      save(`${view.level}-climb`, pic(w, view, m, 3));
      save(`${view.level}-write`, pic(w, view, m, UP + 2));
    }
  });

  test('Reduce Motion: the character appears upstairs, and is back when it wrote', () => {
    const m = new Motion(true);
    m.wrote(ALPHA, CLAUDE, 0);
    expect(m.trip(CLAUDE, 0).where).toBe('loft');
    expect(m.trip(CLAUDE, FRAMES.write - 1).where).toBe('loft');
    expect(m.trip(CLAUDE, FRAMES.write).where).toBe('room');
    // No climb and no slide.
    expect(m.running('climb', CLAUDE, 0)).toBe(false);
    expect(m.progress('note', ALPHA, 0)).toBe(1);
    // It stands still upstairs: the same picture at each frame of its stay.
    expect(pic(w, loft, m, 0)).not.toBe(rest(w, loft, 0, true));
    expect(pic(w, loft, m, 0)).toBe(pic(w, loft, m, 5));
    save('reduce-loft', pic(w, loft, m, 0));
    expect(pic(w, loft, m, FRAMES.write)).toBe(rest(w, loft, FRAMES.write, true));
    expect(m.active(FRAMES.write)).toBe(false);
  });
});

describe('tools: a call is a spark on the cable', () => {
  const w = demoWorld();
  const view = room(CLAUDE, true);
  const plan = planRoom(W, H, true);
  /** The cells outside the right wall of the room: the cables. */
  const outside = (r: readonly Row[]): string => region(r, plan.x + 40, plan.panelX, 1, H - 1);
  const inside = (r: readonly Row[]): string => region(r, 0, plan.x + 40, 1, H - 1);

  test('a tool on another computer: the spark runs out along its cable for 3 frames', () => {
    const m = new Motion();
    m.called(CLAUDE, 'train', false, 0);
    const frames = [0, 1, 2].map((t) => rows(w, view, m, t));
    const still = (t: number): Row[] => rows(w, view, new Motion(), t);
    // It leaves through the socket in the wall and runs out: each frame a new place, and nothing else in the room changes.
    const wall = plan.x + 37;
    frames.forEach((f, t) => {
      expect(text(f), `frame ${t}`).not.toBe(text(still(t)));
      expect(region(f, 0, wall, 1, H - 1), `frame ${t}`).toBe(region(still(t), 0, wall, 1, H - 1));
    });
    expect(distinct(frames.map((f) => region(f, wall, plan.panelX, 1, H - 1)))).toBe(true);
    expect(outside(frames[2] as Row[])).not.toBe(outside(still(2)));
    save('spark-1', text(frames[1] as Row[]));
    expect(text(rows(w, view, m, 3))).toBe(text(still(3)));
    expect(m.active(3)).toBe(false);
  });

  test('a tool that asks: the spark stops at the cover of its socket, and a card rises', () => {
    const m = new Motion();
    m.called(CLAUDE, 'screenshot', true, 0);
    const still = (t: number): Row[] => rows(w, view, new Motion(), t);
    for (const t of [0, 1, 2]) {
      const f = rows(w, view, m, t);
      // Nothing passes the wall.
      expect(outside(f), `frame ${t}`).toBe(outside(still(t)));
      expect(inside(f), `frame ${t}`).not.toBe(inside(still(t)));
    }
    // Then the card, for 2 frames, a little higher in the second.
    const card = [FRAMES.spark, FRAMES.spark + 1].map((t) => rows(w, view, m, t));
    card.forEach((f, i) => expect(inside(f), `card ${i}`).not.toBe(inside(still(FRAMES.spark + i))));
    expect(inside(card[0] as Row[])).not.toBe(inside(card[1] as Row[]));
    expect(outside(card[0] as Row[])).toBe(outside(still(FRAMES.spark)));
    save('spark-cover-1', text(rows(w, view, m, 1)));
    save('spark-card', text(card[1] as Row[]));
    const end = FRAMES.spark + FRAMES.rise;
    expect(text(rows(w, view, m, end))).toBe(text(still(end)));
    expect(m.active(end)).toBe(false);
  });

  test('a tool in the room: the spark runs along the cable to its appliance', () => {
    const m = new Motion();
    m.called(CLAUDE, 'lint', false, 0);
    for (const t of [0, 1, 2]) expect(pic(w, view, m, t), `frame ${t}`).not.toBe(rest(w, view, t));
    expect(pic(w, view, m, 3)).toBe(rest(w, view, 3));
  });

  test('a tool on your machine: its plug in the building and its cable on the ranch', () => {
    for (const v of [{ ...building, sel: `room:${CLAUDE}` }, ranch]) {
      const m = new Motion();
      m.called(CLAUDE, 'screenshot', true, 0);
      expect(pic(w, v, m, 1), v.level).not.toBe(rest(w, v, 1));
      const end = FRAMES.spark + FRAMES.rise;
      expect(pic(w, v, m, end), v.level).toBe(rest(w, v, end));
    }
  });

  test('Reduce Motion: no spark. The cable is bright for one second', () => {
    const m = new Motion(true);
    m.called(CLAUDE, 'train', false, 0);
    expect(m.running('spark', `${CLAUDE}:train`, 0)).toBe(false);
    const bright = pic(w, view, m, 0);
    expect(bright).not.toBe(rest(w, view, 0, true));
    // Nothing travels: the cable is the same for all of its 8 frames.
    const cable = (t: number, motion: Motion): string => outside(rows(w, view, motion, t));
    for (let t = 1; t < FRAMES.glow; t++) expect(cable(t, m), `frame ${t}`).toBe(cable(0, m));
    expect(cable(FRAMES.glow, m)).toBe(cable(FRAMES.glow, new Motion(true)));
    save('reduce-call', bright);
    expect(m.active(FRAMES.glow - 1)).toBe(true);
    expect(m.active(FRAMES.glow)).toBe(false);
  });
});

describe('shared notes: a tag flips', () => {
  const w = demoWorld();

  test('on the pegboard of a room, at the gate and on the fence of a yard: 2 frames', () => {
    const cases: [View, string][] = [
      [room(CLAUDE, true), 'gate'],
      [room(CLAUDE, true), `yard:${ALPHA}:team`],
      [ranch, 'gate'],
      [ranch, `yard:${ALPHA}:team`],
    ];
    for (const [view, id] of cases) {
      const m = new Motion();
      m.start('flip', id, 0);
      const frames = [0, 1].map((t) => pic(w, view, m, t));
      frames.forEach((f, t) => expect(f, `${view.level} ${id} ${t}`).not.toBe(rest(w, view, t)));
      // The tag shows its edge low, then high: the two frames differ.
      expect(frames[0], `${view.level} ${id}`).not.toBe(frames[1]);
      expect(pic(w, view, m, 2), `${view.level} ${id}`).toBe(rest(w, view, 2));
      expect(m.active(2)).toBe(false);
      if (view.level === 'room' && id === 'gate') save('flip-0', frames[0] as string);
    }
  });

  test('Reduce Motion: a dot for two seconds', () => {
    const view = room(CLAUDE, true);
    const m = new Motion(true);
    m.start('flip', 'gate', 0);
    expect(m.running('flip', 'gate', 0)).toBe(false);
    const dot = pic(w, view, m, 0);
    expect(dot).not.toBe(rest(w, view, 0, true));
    const plan = planRoom(W, H, true);
    const board = (t: number, motion: Motion): string => region(rows(w, view, motion, t), plan.x + 16, plan.x + 27, 1, Math.ceil((plan.y + 7) / 2));
    for (let t = 1; t < FRAMES.dot; t++) expect(board(t, m), `frame ${t}`).toBe(board(0, m));
    expect(board(FRAMES.dot, m)).toBe(board(FRAMES.dot, new Motion(true)));
    save('reduce-note', dot);
    expect(m.active(FRAMES.dot)).toBe(false);
  });
});

describe('the yard, the helper and the island', () => {
  test('a room joins: the fence extends in 4 frames. Reduce Motion: it fades in', () => {
    const before = demoWorld((d) => {
      for (const s of d.ps.sandboxes) if (s.id === CODEX) delete s.pasture;
    });
    const after = demoWorld((d) => {
      for (const s of d.ps.sandboxes) if (s.id === GEMINI) s.pasture = 'team';
    });
    const m = new Motion();
    observe(m, demoWorld(), after, 0);
    const frames = [0, 1, 2, 3].map((t) => pic(after, ranch, m, t));
    frames.forEach((f, t) => expect(f, `frame ${t}`).not.toBe(rest(after, ranch, t)));
    save('fence-1', frames[1] as string);
    expect(pic(after, ranch, m, 4)).toBe(rest(after, ranch, 4));
    // Reduce Motion: one frame in which the new piece is half there.
    const r = new Motion(true);
    observe(r, before, demoWorld(), 0);
    expect(r.fade('join', `team:${CODEX}`, 0)).toBe(0.5);
    expect(pic(demoWorld(), ranch, r, 0)).not.toBe(rest(demoWorld(), ranch, 0, true));
    expect(pic(demoWorld(), ranch, r, 1)).toBe(rest(demoWorld(), ranch, 1, true));
    expect(r.active(0)).toBe(true);
    expect(r.active(1)).toBe(false);
  });

  test('a helper: its room builds in 6 frames, and its envelope travels in 4', () => {
    const alone = demoWorld((d) => {
      d.ps.sandboxes = d.ps.sandboxes.filter((s) => s.id !== GEMINI);
    });
    const m = new Motion();
    observe(m, alone, demoWorld(), 0);
    const frames = Array.from({ length: FRAMES.spawn }, (_, t) => pic(demoWorld(), ranch, m, t));
    frames.forEach((f, t) => expect(f, `frame ${t}`).not.toBe(rest(demoWorld(), ranch, t)));
    save('helper-3', frames[3] as string);
    expect(pic(demoWorld(), ranch, m, FRAMES.spawn)).toBe(rest(demoWorld(), ranch, FRAMES.spawn));
    // It ends: the envelope.
    const ended = demoWorld((d) => {
      d.ps.sandboxes = d.ps.sandboxes.filter((s) => s.id !== GEMINI);
      d.ps.ended?.push({ id: GEMINI, short: '0a1a2a', harness: 'gemini', project: ALPHA, ended: T, status: 'ok' });
    });
    const k = new Motion();
    observe(k, demoWorld(), ended, 0);
    const way = [0, 1, 2, 3].map((t) => pic(ended, ranch, k, t));
    way.forEach((f, t) => expect(f, `envelope ${t}`).not.toBe(rest(ended, ranch, t)));
    expect(distinct(way)).toBe(true);
    save('envelope-2', way[2] as string);
    expect(pic(ended, ranch, k, FRAMES.end)).toBe(rest(ended, ranch, FRAMES.end));
    // Reduce Motion: the room and its path fade in, in one frame.
    const r = new Motion(true);
    observe(r, alone, demoWorld(), 0);
    expect(pic(demoWorld(), ranch, r, 0)).not.toBe(rest(demoWorld(), ranch, 0, true));
    expect(pic(demoWorld(), ranch, r, 1)).toBe(rest(demoWorld(), ranch, 1, true));
    save('reduce-helper', pic(demoWorld(), ranch, r, 0));
  });

  test('the fan of the island turns once in 2 s while a job runs. Reduce Motion: a lit window', () => {
    // Its 4 pictures are half a turn: each stays for 2 frames, and a turn shows them twice.
    const turn = Array.from({ length: FAN_LOOP }, (_, t) => fanAt(t));
    expect(new Set(turn).size).toBe(4);
    for (let t = 0; t < FAN_LOOP; t += 2) expect(turn[t]).toBe(turn[t + 1]);
    for (let t = 0; t + 2 < FAN_LOOP; t += 2) expect(turn[t]).not.toBe(turn[t + 2]);
    expect(fanAt(FAN_LOOP)).toBe(fanAt(0));
    // A job runs on gpu-1 (the instance of `train`): the picture moves. Without a job, the island is still.
    const busy = demoWorld((d) => {
      // Only the island can move: every run is paused.
      for (const s of d.ps.sandboxes) s.frozen_since = T - 60;
      d.pending = [];
    });
    const idle = demoWorld((d) => {
      for (const s of d.ps.sandboxes) {
        s.frozen_since = T - 60;
        s.devices = [];
      }
      d.pending = [];
    });
    const draw = (world: World, tick: number, motion = new Motion()) => drawWorld(world, { w: W, h: H, mode: 'truecolor', tick, motion, view: ranch, t: T });
    expect(draw(busy, 0).loops).toBe(true);
    expect(pic(busy, ranch, new Motion(), 0)).not.toBe(pic(busy, ranch, new Motion(), 2));
    expect(pic(busy, ranch, new Motion(), 0)).toBe(pic(busy, ranch, new Motion(), FAN_LOOP));
    save('fan-0', pic(busy, ranch, new Motion(), 0));
    save('fan-2', pic(busy, ranch, new Motion(), 2));
    expect(draw(idle, 0).loops).toBe(false);
    // Reduce Motion: nothing loops, and the place of the fan is a lit window.
    expect(draw(busy, 0, new Motion(true)).loops).toBe(false);
    expect(pic(busy, ranch, new Motion(true), 0)).toBe(pic(busy, ranch, new Motion(true), 5));
    expect(pic(busy, ranch, new Motion(true), 0)).not.toBe(pic(idle, ranch, new Motion(true), 0));
    save('reduce-fan', pic(busy, ranch, new Motion(true), 0));
  });
});

describe('the camera', () => {
  const w = demoWorld();
  const opts = (motion: Motion, o: Partial<Parameters<typeof worldScene>[2]> = {}) => ({ w: W, h: H, mode: 'truecolor' as const, motion, t: T, ...o });
  const ranchRows = (): readonly Row[] => worldScene(w, ranch, opts(new Motion())).rows(0);
  const cell = (r: readonly Row[], x: number, y: number): string => `${r[y]?.ch[x]}|${r[y]?.st[x]}`;

  test('ranch to building: the building opens over the ranch, from the thing that was opened', () => {
    const before = ranchRows();
    const m = new Motion();
    m.start('zoom', 'world', 0);
    const from = { x: 40, y: 10, w: 12, h: 6 };
    const s = worldScene(w, building, opts(m, { zoomFrom: from, before }));
    const end = worldScene(w, building, opts(new Motion())).rows(FRAMES.zoom);
    const first = s.rows(0);
    // At its start the ranch is still there around the place of the thing, and the building is inside it.
    expect(cell(first, 2, 5)).toBe(cell(before, 2, 5));
    expect(cell(first, 45, 12)).toBe(cell(end, 45, 12));
    // The top line is the new one at once: you always see where you are.
    expect(first[0]?.ch.join('')).toContain('Ranch › alpha');
    // Each frame shows more of the building.
    const same = (r: readonly Row[]): number => r.reduce((n, row, y) => n + row.ch.filter((_, x) => cell(r, x, y) === cell(end, x, y)).length, 0);
    const steps = [0, 1, 2, 3, 4].map((t) => same(s.rows(t)));
    for (let i = 1; i < steps.length; i++) expect(steps[i], `frame ${i}`).toBeGreaterThan(steps[i - 1] as number);
    save('zoom-1', text(s.rows(1)));
    expect(s.rows(FRAMES.zoom)).toEqual(end);
    expect(s.animated(1)).toBe(true);
  });

  test('one stop up: the old picture closes on the thing that it was', () => {
    const before = worldScene(w, building, opts(new Motion())).rows(0);
    const m = new Motion();
    m.start('zoom', 'world', 0, 'out');
    const view = { ...ranch, sel: `room:${CLAUDE}` };
    const s = worldScene(w, view, opts(m, { before }));
    const end = worldScene(w, view, opts(new Motion())).rows(FRAMES.zoom);
    // At its start the building still fills the picture, under the new top line.
    expect(s.rows(0).slice(1)).toEqual(before.slice(1));
    expect(s.rows(0)[0]).toEqual(end[0]);
    const old = (r: readonly Row[]): number => r.slice(1).reduce((n, row, y) => n + row.ch.filter((_, x) => cell(r, x, y + 1) === cell(before, x, y + 1) && cell(before, x, y + 1) !== cell(end, x, y + 1)).length, 0);
    const steps = [0, 1, 2, 3].map((t) => old(s.rows(t)));
    for (let i = 1; i < steps.length; i++) expect(steps[i], `frame ${i}`).toBeLessThan(steps[i - 1] as number);
    save('zoom-out-1', text(s.rows(1)));
    expect(s.rows(FRAMES.zoom)).toEqual(end);
  });

  test('room to loft: the loft comes down from the top and the room leaves at the bottom. Back: the reverse', () => {
    const before = worldScene(w, room(CLAUDE), opts(new Motion())).rows(0);
    const m = new Motion();
    m.start('pan', 'world', 0);
    const s = worldScene(w, loft, opts(m, { before }));
    const end = worldScene(w, loft, opts(new Motion())).rows(FRAMES.pan);
    // At its start: the room. Then the last rows of the loft are at the top, and the room is lower.
    expect(s.rows(0).slice(1)).toEqual(before.slice(1));
    const one = s.rows(1);
    const d = one.findIndex((r, y) => y > 0 && r === before[1]) - 1;
    expect(d).toBeGreaterThan(0);
    expect(one[d]).toEqual(end[H - 1]);
    expect(one[1]).toEqual(end[H - d]);
    expect(one[H - 1]).toBe(before[H - 1 - d]);
    save('pan-1', text(one));
    expect(s.rows(FRAMES.pan)).toEqual(end);
    expect(s.animated(FRAMES.pan)).toBe(false);
    // Down again.
    const k = new Motion();
    k.start('pan', 'world', 0, 'down');
    const back = worldScene(w, room(CLAUDE), opts(k, { before: end }));
    const home = worldScene(w, room(CLAUDE), opts(new Motion())).rows(1);
    const down = back.rows(1);
    // The loft goes up and out, and the room comes in from the bottom: its first row is under the last row of the loft that still shows.
    const e = down.findIndex((r, y) => y > 1 && r !== end[y] && !end.includes(r));
    expect(e).toBeGreaterThan(1);
    expect(down[e]).toEqual(home[1]);
    expect(down[1]).toBe(end[1 + (H - e)]);
    expect(down[H - 1]).toEqual(home[H - e]);
    expect(back.rows(FRAMES.pan)).toEqual(worldScene(w, room(CLAUDE), opts(new Motion())).rows(FRAMES.pan));
  });

  test('Plumbing: the walls turn to glass in 3 frames, and back', () => {
    for (const level of ['room', 'building', 'ranch'] as const) {
      const view = level === 'room' ? room(CLAUDE) : level === 'building' ? building : ranch;
      const on = { ...view, plumbing: true };
      const m = new Motion();
      m.start('plumb', 'world', 0);
      const frames = [0, 1, 2].map((t) => pic(w, on, m, t));
      frames.forEach((f, t) => expect(f, `${level} ${t}`).not.toBe(rest(w, on, t)));
      expect(distinct(frames)).toBe(true);
      expect(pic(w, on, m, 3), level).toBe(rest(w, on, 3));
      if (level === 'room') save('glass-1', frames[1] as string);
      // Back: the glass turns to walls.
      const k = new Motion();
      k.start('unplumb', 'world', 0);
      const back = [0, 1, 2].map((t) => pic(w, view, k, t));
      back.forEach((f, t) => expect(f, `${level} back ${t}`).not.toBe(rest(w, view, t)));
      expect(pic(w, view, k, 3), `${level} back`).toBe(rest(w, view, 3));
      expect(k.active(3)).toBe(false);
    }
  });

  test('Reduce Motion: every transition is one frame of cross-fade, 150 ms', () => {
    for (const kind of ['zoom', 'pan', 'plumb', 'unplumb'] as const) {
      expect(REDUCED[kind], kind).toBe('fade');
      const before = ranchRows();
      const m = new Motion(true);
      m.start(kind, 'world', 0);
      expect(m.running(kind, 'world', 0), kind).toBe(false);
      const s = worldScene(w, building, opts(m, { before }));
      const end = worldScene(w, building, opts(new Motion(true))).rows(0);
      const mid = s.rows(0);
      // Nothing moved: the one frame is between the two pictures, cell by cell.
      expect(mid, kind).not.toEqual(before);
      expect(mid, kind).not.toEqual(end);
      expect(mid[0], kind).toEqual(end[0]);
      expect(s.animated(0), kind).toBe(true);
      expect(s.rows(1), kind).toEqual(worldScene(w, building, opts(new Motion(true))).rows(1));
      if (kind === 'zoom') save('reduce-fade', text(mid));
    }
    // The mix of one cell: a grass pixel and a wall pixel give the colour between them.
    const a: Row[] = [{ ch: [' ', '▀', 'x'], st: ['48;2;0;0;0', '38;2;0;0;0;48;2;100;100;100', ''] }];
    const b: Row[] = [{ ch: [' ', ' ', 'y'], st: ['48;2;200;100;50', '48;2;200;200;200', '1;38;2;255;255;255;48;2;0;0;0'] }];
    const mixed = crossfade(a, b, 0.5);
    expect(mixed[0]?.st[0]).toBe('48;2;100;50;25');
    expect(mixed[0]?.ch[1]).toBe('▀');
    expect(mixed[0]?.st[1]).toBe('38;2;100;100;100;48;2;150;150;150');
    // A word is the new word at once.
    expect(mixed[0]?.ch[2]).toBe('y');
    expect(crossfade(a, b, 1)).toBe(b);
    // A picture of another size cannot be mixed: the new one shows.
    expect(crossfade([], b, 0.5)).toBe(b);
  });

  test('with the 256 colours a cell cannot be mixed: the new picture shows at once', () => {
    const before = worldScene(w, ranch, opts(new Motion(), { mode: '256' })).rows(0);
    const m = new Motion(true);
    m.start('zoom', 'world', 0);
    const s = worldScene(w, building, opts(m, { before, mode: '256' }));
    expect(s.rows(0)).toEqual(worldScene(w, building, opts(new Motion(true), { mode: '256' })).rows(0));
  });
});

describe('who gets Reduce Motion', () => {
  const none: ReadCommand = () => null;

  test('the flag, then PADDOCK_TUI_MOTION, then the desktop', () => {
    expect(wantsReducedMotion(true, {}, 'linux', none)).toBe(true);
    expect(wantsReducedMotion(false, { PADDOCK_TUI_MOTION: 'reduce' }, 'linux', none)).toBe(true);
    expect(wantsReducedMotion(false, {}, 'linux', none)).toBe(false);
    expect(['full', 'on', '1', 'FULL'].map((v) => fullMotion({ PADDOCK_TUI_MOTION: v }))).toEqual([true, true, true, true]);
    expect(fullMotion({})).toBe(false);
    // The desktop says reduce: the world follows, and PADDOCK_TUI_MOTION=full says no.
    const mac: ReadCommand = (cmd, args) => (cmd === 'defaults' && args.join(' ') === 'read com.apple.universalaccess reduceMotion' ? '1\n' : null);
    expect(wantsReducedMotion(false, {}, 'darwin', mac)).toBe(true);
    expect(wantsReducedMotion(false, { PADDOCK_TUI_MOTION: 'full' }, 'darwin', mac)).toBe(false);
    expect(desktopReducedMotion({}, 'darwin', () => '0\n')).toBe(false);
  });

  test('GNOME and KDE, and no guess where nothing is known', () => {
    const calls: string[] = [];
    const gnome: ReadCommand = (cmd, args) => (calls.push(cmd), cmd === 'gsettings' && args[2] === 'enable-animations' ? 'false\n' : null);
    expect(desktopReducedMotion({ XDG_CURRENT_DESKTOP: 'ubuntu:GNOME' }, 'linux', gnome)).toBe(true);
    expect(desktopReducedMotion({ XDG_CURRENT_DESKTOP: 'GNOME' }, 'linux', () => 'true\n')).toBe(false);
    expect(desktopReducedMotion({ XDG_CURRENT_DESKTOP: 'KDE' }, 'linux', (cmd) => (cmd === 'kreadconfig6' ? '0\n' : null))).toBe(true);
    expect(desktopReducedMotion({ XDG_CURRENT_DESKTOP: 'KDE' }, 'linux', (cmd) => (cmd === 'kreadconfig5' ? '1.0\n' : null))).toBe(false);
    expect(desktopReducedMotion({ XDG_CURRENT_DESKTOP: 'KDE' }, 'linux', () => '')).toBe(null);
    // Another desktop, another system, a command that fails: not known, so motion stays.
    calls.length = 0;
    const count: ReadCommand = (cmd) => (calls.push(cmd), null);
    expect(desktopReducedMotion({ XDG_CURRENT_DESKTOP: 'Hyprland' }, 'linux', count)).toBe(null);
    expect(desktopReducedMotion({}, 'win32', count)).toBe(null);
    expect(calls).toEqual([]);
    expect(desktopReducedMotion({}, 'darwin', count)).toBe(null);
    // Over SSH the desktop of this machine is not the one of the terminal: no command runs.
    calls.length = 0;
    expect(desktopReducedMotion({ SSH_CONNECTION: '10.0.0.1 1 10.0.0.2 22', XDG_CURRENT_DESKTOP: 'GNOME' }, 'linux', count)).toBe(null);
    expect(calls).toEqual([]);
  });

  test('NO_COLOR: no art, so no clock and no motion at all', () => {
    expect(detectArt({ NO_COLOR: '1', COLORTERM: 'truecolor', TERM: 'xterm-kitty' }, true)).toBe('none');
    const layer = new ArtLayer({ mode: 'none', sink: () => undefined, screen: () => ({ columns: W, rows: H }) });
    expect(layer.animates).toBe(false);
    // The interface sets this from the art layer (app.tsx).
    const m = new Motion();
    m.still = !layer.animates;
    observe(m, demoWorld((d) => (d.pending = [])), demoWorld(), 0);
    m.start('zoom', 'world', 0);
    m.wrote(ALPHA, CLAUDE, 0);
    m.called(CLAUDE, 'train', true, 0);
    m.end(GEMINI, CLAUDE, 0);
    expect(m.active(0)).toBe(false);
    expect(m.trip(CLAUDE, 0).where).toBe('room');
    // The world in words is the same at every frame.
    const words = (tick: number): string => text(rows(demoWorld(), ranch, m, tick, 'none'));
    expect(words(0)).toBe(words(3));
    expect(words(0)).not.toContain('\u001b[0;38');
  });
});

describe('no work while nothing moves', () => {
  test('the clock runs for the frames of a motion, writes only the cells that change, and stops', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    const writes: string[] = [];
    const layer = new ArtLayer({ mode: 'truecolor', sink: (s) => writes.push(s), screen: () => ({ columns: 200, rows: 100 }) });
    const m = new Motion();
    const w = demoWorld();
    // The loft has no loop: only a motion can move it.
    const scene = (): ReturnType<typeof worldScene> => worldScene(w, loft, { w: W, h: H, mode: 'truecolor', motion: m, t: T, tick: layer.tick });
    const tree = (): React.JSX.Element => (
      <ArtContext.Provider value={layer}>
        <SceneView scene={scene()} />
      </ArtContext.Provider>
    );
    const inst = render(tree());
    await new Promise((r) => setTimeout(r, 20));
    // Idle: no timer, and a minute passes with no step and no byte.
    expect(layer.running).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(60_000);
    expect(layer.steps).toBe(0);
    expect(writes).toHaveLength(0);

    // A note is written: the next render starts the clock.
    m.wrote(ALPHA, CLAUDE, layer.tick);
    inst.rerender(tree());
    await new Promise((r) => setTimeout(r, 20));
    expect(layer.running).toBe(true);
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(60_000);
    // It ran for the frames of the trip, then it stopped by itself.
    expect(layer.running).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    expect(layer.tick).toBe(LOFT_VISIT);
    expect(writes.length).toBeGreaterThan(FRAMES.climb);
    expect(writes.length).toBeLessThanOrEqual(LOFT_VISIT);
    // Each step is a patch: far fewer cells than the picture has (100 x 31 = 3100).
    for (const s of writes) {
      expect(s.startsWith('\u001b7')).toBe(true);
      expect((s.match(/[▀▄ ]/g) ?? []).length).toBeLessThan(700);
    }
    // After the motion: nothing more, however long.
    const n = writes.length;
    vi.advanceTimersByTime(600_000);
    expect(writes.length).toBe(n);
    expect(layer.tick).toBe(LOFT_VISIT);

    // In the background a motion rests, with no timer, and goes on where it was.
    m.called(CLAUDE, 'train', false, layer.tick);
    m.start('zoom', 'world', layer.tick);
    layer.setFocused(false);
    inst.rerender(tree());
    await new Promise((r) => setTimeout(r, 20));
    expect(layer.running).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(60_000);
    expect(layer.tick).toBe(LOFT_VISIT);
    expect(writes.length).toBe(n);
    layer.setFocused(true);
    expect(layer.running).toBe(true);
    vi.advanceTimersByTime(60_000);
    expect(layer.tick).toBe(LOFT_VISIT + FRAMES.zoom);
    expect(layer.running).toBe(false);
  });

  test('Reduce Motion: one step for a cross-fade, then nothing', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    const writes: string[] = [];
    const layer = new ArtLayer({ mode: 'truecolor', sink: (s) => writes.push(s), screen: () => ({ columns: 200, rows: 100 }) });
    const m = new Motion(true);
    const w = demoWorld();
    const before = worldScene(w, room(GROK), { w: W, h: H, mode: 'truecolor', motion: m, t: T }).rows(0);
    m.start('pan', 'world', 0);
    // Changes that are instant swaps keep no clock.
    m.start('freeze', GROK, 0);
    m.start('ask', CODEX, 0);
    m.start('net', CODEX, 0);
    m.start('climb', CLAUDE, 0);
    render(
      <ArtContext.Provider value={layer}>
        <SceneView scene={worldScene(w, loft, { w: W, h: H, mode: 'truecolor', motion: m, t: T, before, tick: 0 })} />
      </ArtContext.Provider>,
    );
    await new Promise((r) => setTimeout(r, 20));
    expect(layer.running).toBe(true);
    vi.advanceTimersByTime(60_000);
    expect(layer.tick).toBe(1);
    expect(writes).toHaveLength(1);
    expect(layer.running).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('the camera in the interface', () => {
  const strip = (s: string): string => s.replace(/\u001b\[[\d;]*m/g, '');
  const crumb = (r: Rendered): string => strip(r.lines()[1] ?? '');
  /** A clock that the test steps by hand: its own timer fires once in a long while. */
  const layerOf = (): ArtLayer => new ArtLayer({ mode: 'truecolor', sink: () => undefined, fps: 0.001, screen: () => ({ columns: 100, rows: 30 }) });

  async function ranchOf(layer: ArtLayer, reduceMotion = false): Promise<Rendered> {
    fakeWorld();
    const r = await renderApp({ columns: 100, rows: 30, art: layer, reduceMotion });
    await r.waitFor((f) => strip(f).includes('Runs · 4 live'));
    await r.press('b');
    await r.waitFor((f) => strip(f).split('\n')[1]?.startsWith('Ranch ') === true && strip(f).includes('ask you'));
    return r;
  }

  test('Enter: the building opens over the ranch. Esc: it closes on its room. P: the walls turn to glass and back', async () => {
    const layer = layerOf();
    const r = await ranchOf(layer);
    expect(layer.animates).toBe(true);
    const start = layer.tick;
    // A word that only the ranch has: the sign of the other project.
    expect(strip(r.frame())).toContain(' beta ');
    await r.press('\r');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha '));
    // The first frame of the zoom: the clock did not step, and the ranch is still around the building.
    expect(layer.tick).toBe(start);
    expect(strip(r.frame())).toContain(' beta ');
    for (let i = 0; i < FRAMES.zoom; i++) layer.step();
    // A new render of the scene after the zoom: the building alone.
    await r.press('i');
    await r.waitFor((f) => strip(f).includes('Inspector'));
    await r.press('i');
    await r.waitFor((f) => !strip(f).includes('Inspector'));
    expect(strip(r.frame())).not.toContain(' beta ');
    const built = strip(r.frame());
    expect(built).toContain('› codex-0b0b0b ◆ asks you');

    // Plumbing on: the first frame has the walls that it had, the last one has glass.
    await r.press('p');
    await r.waitFor(() => crumb(r).includes('Plumbing'));
    const first = r.frame();
    for (let i = 0; i < FRAMES.plumb; i++) layer.step();
    await r.press('i');
    await r.waitFor((f) => strip(f).includes('Inspector'));
    await r.press('i');
    await r.waitFor((f) => !strip(f).includes('Inspector'));
    expect(r.frame()).not.toBe(first);
    // And off: the walls come back over the same 3 frames.
    await r.press('p');
    await r.waitFor(() => !crumb(r).includes('Plumbing'));
    const glass = r.frame();
    for (let i = 0; i < FRAMES.unplumb; i++) layer.step();
    await r.press('i');
    await r.waitFor((f) => strip(f).includes('Inspector'));
    await r.press('i');
    await r.waitFor((f) => !strip(f).includes('Inspector'));
    expect(r.frame()).not.toBe(glass);

    // One stop up: at the first frame the building still fills the picture, under the top line of the ranch.
    await r.press('\u001b');
    await r.waitFor(() => /^Ranch +◆/.test(crumb(r)));
    expect(strip(r.frame())).toContain('› codex-0b0b0b ◆ asks you');
    expect(strip(r.frame())).not.toContain(' beta ');
    for (let i = 0; i < FRAMES.zoom; i++) layer.step();
    await r.press('\u001b[C');
    await r.waitFor((f) => strip(f).includes(' beta '));
    expect(strip(r.frame())).not.toContain('› codex-0b0b0b ◆ asks you');
  });

  test('Reduce Motion: no zoom. The new picture is there at once, in one frame of cross-fade', async () => {
    const layer = layerOf();
    const r = await ranchOf(layer, true);
    const start = layer.tick;
    await r.press('\r');
    await r.waitFor(() => crumb(r).startsWith('Ranch › alpha '));
    expect(layer.tick).toBe(start);
    // The words of the building, and none of the ranch.
    const faded = r.frame();
    expect(strip(faded)).not.toContain(' beta ');
    expect(strip(faded)).toContain('› codex-0b0b0b ◆');
    // One step later the picture has its own colours.
    layer.step();
    await r.press('i');
    await r.waitFor((f) => strip(f).includes('Inspector'));
    await r.press('i');
    await r.waitFor((f) => !strip(f).includes('Inspector'));
    expect(strip(r.frame())).toContain('› codex-0b0b0b ◆ asks you');
    expect(r.frame()).not.toBe(faded);
  });
});
