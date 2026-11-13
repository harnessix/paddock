// The motion of the world at 8 frames a second: what starts a motion,
// how far it is at each frame, that it rests in the background, and that
// reduced motion shows the end state at once.
import React from 'react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { cleanup, render } from 'ink-testing-library';
import { ArtContext, ArtLayer, drawRowChanges, FPS, SceneView, type Scene } from '../src/art-layer.js';
import { Painter, type Row } from '../src/screen.js';
import { ease, FRAMES, lerp, Motion } from '../src/world/motion.js';
import { observe } from '../src/world/observe.js';
import { AMBER, FROST } from '../src/world/pixels.js';
import { drawWorld, type View } from '../src/world/scene.js';
import { defaultSelection, worldScene } from '../src/views/world.js';
import type { World } from '../src/world/model.js';
import { CLAUDE, CODEX, demoWorld, GEMINI, GROK, T } from './world-demo.js';

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

const ALPHA = '/src/alpha';
const roomView = (room: string): View => ({ level: 'room', house: ALPHA, room, sel: '', plumbing: false });
const ranch: View = { level: 'ranch', house: '', room: '', sel: '', plumbing: false };

/** How many cells of a picture have a colour. */
function count(world: World, view: View, motion: Motion, tick: number, rgb: readonly number[]): number {
  const code = `2;${rgb[0]};${rgb[1]};${rgb[2]}`;
  let n = 0;
  for (const r of drawWorld(world, { w: 80, h: 21, mode: 'truecolor', tick, motion, view, t: T }).canvas.rows()) for (const s of r.st) if (s.includes(code)) n++;
  return n;
}

function picture(world: World, view: View, motion: Motion, tick: number): string {
  return drawWorld(world, { w: 80, h: 21, mode: 'truecolor', tick, motion, view, t: T })
    .canvas.rows()
    .map((r) => r.ch.join('') + r.st.join('|'))
    .join('\n');
}

describe('the clock of a motion', () => {
  test('the lengths of the design, in frames of 125 ms', () => {
    expect(FPS).toBe(8);
    const ms = (k: keyof typeof FRAMES): number => (FRAMES[k] * 1000) / FPS;
    // Each within one frame of the time that the design gives.
    const design: [keyof typeof FRAMES, number][] = [
      ['ask', 500],
      ['answer', 400],
      ['freeze', 600],
      ['thaw', 600],
      ['net', 300],
      ['mail', 200],
      ['spawn', 800],
      ['end', 500],
      ['join', 500],
      ['note', 250],
      ['plumb', 300],
      ['zoom', 450],
      ['pan', 350],
    ];
    for (const [k, want] of design) expect(Math.abs(ms(k) - want), k).toBeLessThanOrEqual(125);
  });

  test('a motion goes from 0 to 1 over its frames, then it is over and forgotten', () => {
    const m = new Motion();
    expect(m.progress('ask', 'a', 0)).toBe(1);
    expect(m.active(0)).toBe(false);
    m.start('ask', 'a', 10);
    expect([10, 11, 12, 13, 14, 15].map((t) => m.progress('ask', 'a', t))).toEqual([0, 0.25, 0.5, 0.75, 1, 1]);
    expect(m.running('ask', 'a', 13)).toBe(true);
    expect(m.running('ask', 'a', 14)).toBe(false);
    expect(m.running('ask', 'b', 12)).toBe(false);
    expect(m.active(13)).toBe(true);
    expect(m.active(14)).toBe(false);
    expect(ease(0)).toBe(0);
    expect(ease(1)).toBe(1);
    expect(ease(0.5)).toBe(0.75);
    expect(lerp(10, 20, 0.5)).toBe(15);
    expect(lerp(10, 20, 7)).toBe(20);
  });

  test('an envelope travels to the run that started the helper', () => {
    const m = new Motion();
    m.end('child', 'parent', 4);
    expect(m.ended('parent', 4)).toEqual([0]);
    expect(m.ended('parent', 6)).toEqual([0.5]);
    expect(m.ended('other', 6)).toEqual([]);
    expect(m.active(7)).toBe(true);
    expect(m.ended('parent', 8)).toEqual([]);
    expect(m.active(8)).toBe(false);
  });

  test('reduced motion: every motion is over when it starts', () => {
    const m = new Motion(true);
    m.start('freeze', 'a', 0);
    m.end('c', 'p', 0);
    expect(m.progress('freeze', 'a', 0)).toBe(1);
    expect(m.ended('p', 1)).toEqual([]);
    expect(m.active(0)).toBe(false);
  });
});

describe('what starts a motion', () => {
  const started = (prev: World | null, next: World): Record<string, boolean> => {
    const m = new Motion();
    observe(m, prev, next, 0);
    const out: Record<string, boolean> = {};
    for (const [kind, id] of [
      ['pulse', 'world'],
      ['ask', CLAUDE],
      ['answer', CODEX],
      ['freeze', CLAUDE],
      ['thaw', GROK],
      ['net', CODEX],
      ['mail', CODEX],
      ['spawn', 'new-helper'],
      ['join', `team:${GEMINI}`],
      ['slot', '3'],
    ] as const) {
      if (m.running(kind, id, 0)) out[`${kind}:${id}`] = true;
    }
    if (m.ended(CLAUDE, 0).length > 0) out.envelope = true;
    return out;
  };

  test('the first look and the same world again start nothing', () => {
    expect(started(null, demoWorld())).toEqual({});
    expect(started(demoWorld(), demoWorld())).toEqual({});
  });

  test('each change of the JSON starts its motion', () => {
    const next = demoWorld((d) => {
      // A request for claude, and the one of codex is answered.
      d.pending = [{ id: 'r-9', sandbox: CLAUDE, summary: 'rm -rf build' }];
      for (const s of d.ps.sandboxes) {
        if (s.id === CLAUDE) s.frozen_since = T;
        if (s.id === GROK) delete s.frozen_since;
        if (s.id === CODEX) s.network = 'egress';
        if (s.id === GEMINI) s.pasture = 'team';
      }
      d.mail.push({ id: CODEX, state: 'running', unread_up: 1 });
      d.ps.sandboxes.push({ id: 'new-helper', short: 'aaaaaa', harness: 'codex', project: ALPHA, network: 'none', started: T, parent: CLAUDE });
      d.board.compile_slots?.push({ slot: 3, holder: 'codex-alpha-0b0b0b' });
    });
    expect(started(demoWorld(), next)).toEqual({
      [`ask:${CLAUDE}`]: true,
      [`answer:${CODEX}`]: true,
      [`freeze:${CLAUDE}`]: true,
      [`thaw:${GROK}`]: true,
      [`net:${CODEX}`]: true,
      [`mail:${CODEX}`]: true,
      'spawn:new-helper': true,
      [`join:team:${GEMINI}`]: true,
      'slot:3': true,
    });
  });

  test('a helper that ends sends its envelope to the run that started it', () => {
    const next = demoWorld((d) => {
      d.ps.sandboxes = d.ps.sandboxes.filter((s) => s.id !== GEMINI);
      d.ps.ended?.push({ id: GEMINI, short: '0a1a2a', harness: 'gemini', project: ALPHA, ended: T, status: 'ok' });
    });
    expect(started(demoWorld(), next)).toEqual({ envelope: true });
  });
});

describe('the motions in the picture', () => {
  test('a request: the character walks to the door in 4 frames, then the card shows', () => {
    const before = demoWorld((d) => {
      d.pending = [];
      if (d.extra.devices) d.extra.devices.devices = [];
    });
    const after = demoWorld((d) => {
      if (d.extra.devices) d.extra.devices.devices = [];
    });
    const m = new Motion();
    observe(m, before, after, 100);
    const frames = [100, 101, 102, 103, 104, 105].map((t) => picture(after, roomView(CODEX), m, t));
    // It moves while it walks, and stands when it is there. The wait loop has 8 frames, so compare with the loop alone.
    const still = [104, 105].map((t) => picture(after, roomView(CODEX), new Motion(), t));
    expect(frames[4]).toBe(still[0]);
    expect(frames[5]).toBe(still[1]);
    expect(frames[0]).not.toBe(picture(after, roomView(CODEX), new Motion(), 100));
    // The card (the amber cells beside the door) shows when the character is at the door.
    const amberAt = (t: number): number => count(after, roomView(CODEX), m, t, AMBER);
    expect(amberAt(100)).toBeLessThan(amberAt(104));
    expect(amberAt(104)).toBe(amberAt(110));
  });

  test('a freeze: frost grows from the corners over 5 frames, and the character keeps its pose', () => {
    const before = demoWorld((d) => {
      for (const s of d.ps.sandboxes) if (s.id === GROK) delete s.frozen_since;
    });
    const after = demoWorld();
    const m = new Motion();
    observe(m, before, after, 0);
    const frost = [0, 1, 2, 3, 4, 5, 6].map((t) => count(after, roomView(GROK), m, t, FROST));
    expect(frost[0]).toBe(0);
    for (let i = 1; i <= 5; i++) expect(frost[i], `frame ${i}`).toBeGreaterThanOrEqual(frost[i - 1] as number);
    expect(frost[5]).toBeGreaterThan(20);
    expect(frost[6]).toBe(frost[5]);
    // Frozen: the picture is the same at every later frame. Nothing loops.
    expect(picture(after, roomView(GROK), m, 20)).toBe(picture(after, roomView(GROK), m, 27));
    expect(drawWorld(after, { w: 80, h: 21, mode: 'truecolor', tick: 20, motion: m, view: roomView(GROK), t: T }).loops).toBe(false);
    // The thaw melts it again.
    const m2 = new Motion();
    observe(m2, after, before, 0);
    const melt = [0, 2, 5].map((t) => count(before, roomView(GROK), m2, t, FROST));
    expect(melt[0]).toBeGreaterThan(melt[1] as number);
    expect(melt[2]).toBe(0);
  });

  test('reduced motion: the end state at once', () => {
    const before = demoWorld((d) => {
      for (const s of d.ps.sandboxes) if (s.id === GROK) delete s.frozen_since;
    });
    const after = demoWorld();
    const m = new Motion(true);
    observe(m, null, after, 0);
    observe(m, before, after, 0);
    expect(m.active(0)).toBe(false);
    expect(count(after, roomView(GROK), m, 0, FROST)).toBe(count(after, roomView(GROK), new Motion(), 99, FROST));
  });

  test('a room that works loops; the first room that opens pulses its door, its window and its ladder once', () => {
    const w = demoWorld();
    const at = (tick: number, motion = new Motion()) => drawWorld(w, { w: 80, h: 21, mode: 'truecolor', tick, motion, view: roomView(CLAUDE), t: T });
    expect(at(0).loops).toBe(true);
    expect(picture(w, roomView(CLAUDE), new Motion(), 0)).not.toBe(picture(w, roomView(CLAUDE), new Motion(), 1));
    // The loop of 8 frames comes round.
    expect(picture(w, roomView(CLAUDE), new Motion(), 0)).toBe(picture(w, roomView(CLAUDE), new Motion(), 8));
    // The pulse can wait for the zoom: it starts at its frame, not before.
    const m = new Motion();
    m.start('pulse', 'world', 4);
    expect(m.running('pulse', 'world', 3)).toBe(false);
    expect(m.active(3)).toBe(true);
    expect(picture(w, roomView(CLAUDE), m, 3)).toBe(picture(w, roomView(CLAUDE), new Motion(), 3));
    expect(picture(w, roomView(CLAUDE), m, 4)).not.toBe(picture(w, roomView(CLAUDE), new Motion(), 4));
    expect(picture(w, roomView(CLAUDE), m, 4 + FRAMES.pulse)).toBe(picture(w, roomView(CLAUDE), new Motion(), 4 + FRAMES.pulse));
  });

  test('no art: nothing moves, the text is the same at every frame', () => {
    const w = demoWorld();
    const m = new Motion();
    const text = (tick: number): string => drawWorld(w, { w: 80, h: 21, mode: 'none', tick, motion: m, view: ranch, t: T }).canvas.lines().join('\n');
    expect(text(0)).toBe(text(5));
  });
});

describe('the scene of a view', () => {
  const opts = (motion: Motion, o: Partial<Parameters<typeof worldScene>[2]> = {}) => ({ w: 80, h: 21, mode: 'truecolor' as const, motion, t: T, ...o });

  test('the selection is a thing of the picture: a room that asks first, the door of a room that asks', () => {
    const w = demoWorld();
    const s = worldScene(w, ranch, opts(new Motion()));
    expect(s.sel).toBe(`room:${CODEX}`);
    expect(worldScene(w, { ...ranch, sel: 'bench' }, opts(new Motion())).sel).toBe('bench');
    expect(worldScene(w, roomView(CODEX), opts(new Motion())).sel).toBe('door');
    expect(worldScene(w, roomView(CLAUDE), opts(new Motion())).sel).toBe('self');
    expect(defaultSelection(w, { ...ranch, sel: 'gone' }, [])).toBe('');
  });

  test('the zoom opens the picture from the thing that was opened, in 4 frames', () => {
    const w = demoWorld();
    const m = new Motion();
    m.start('zoom', 'world', 0);
    const s = worldScene(w, { ...ranch, level: 'building', house: ALPHA }, opts(m, { zoomFrom: { x: 30, y: 8, w: 12, h: 6 } }));
    const filled = (rows: readonly Row[]): number => rows.slice(1).reduce((n, r) => n + r.ch.filter((c, x) => c !== ' ' || r.st[x] !== '').length, 0);
    const steps = [0, 1, 2, 3, 4].map((t) => filled(s.rows(t)));
    for (let i = 1; i < steps.length; i++) expect(steps[i], `frame ${i}`).toBeGreaterThan(steps[i - 1] as number);
    // At its start, only the place of the opened thing shows.
    const first = s.rows(0);
    expect(first[3]?.ch.every((c) => c === ' ')).toBe(true);
    expect(first[10]?.ch.slice(0, 30).every((c) => c === ' ')).toBe(true);
    // The top line stays: you always see where you are.
    expect(first[0]?.ch.join('')).toContain('Ranch › alpha');
    // After the zoom: the whole picture, the same as with no zoom.
    expect(s.rows(4)).toEqual(worldScene(w, { ...ranch, level: 'building', house: ALPHA }, opts(new Motion())).rows(4));
    expect(s.animated(2)).toBe(true);
  });

  test('the pan to the loft comes down from the top in 3 frames', () => {
    const w = demoWorld();
    const m = new Motion();
    m.start('pan', 'world', 0);
    const s = worldScene(w, { ...ranch, level: 'loft', house: ALPHA }, opts(m));
    const end = worldScene(w, { ...ranch, level: 'loft', house: ALPHA }, opts(new Motion())).rows(3);
    expect(s.rows(0)[1]?.ch.every((c) => c === ' ')).toBe(true);
    expect(s.rows(0)).not.toEqual(end);
    expect(s.rows(3)).toEqual(end);
    // The loft has no loop: after the pan nothing moves.
    expect(s.animated(1)).toBe(true);
    expect(s.animated(3)).toBe(false);
  });

  test('the Inspector lies over the picture and names the command', () => {
    const w = demoWorld();
    const s = worldScene(w, { ...roomView(CODEX), sel: 'door' }, opts(new Motion(), { inspector: true, mode: 'none' }));
    const f = s.rows(0).map((r) => r.ch.join('')).join('\n');
    expect(f).toContain('Inspector · c copies');
    expect(f).toContain('paddock: approval request');
    expect(f).toContain('$ paddock approvals --json');
    expect(f).toContain('"id": "r-1"');
    // The room stays beside it.
    expect(f).toContain('▛▀▀╡▲ loft╞');
    const ranchScene = worldScene(w, { ...ranch, sel: 'bench' }, opts(new Motion(), { inspector: true, mode: 'none', w: 120, h: 27 }));
    expect(ranchScene.rows(0).map((r) => r.ch.join('')).join('\n')).toContain('paddock: compile slots');
  });
});

describe('the clock of the art layer', () => {
  function still(rows: Row[]): Scene & { moving: boolean } {
    return { w: rows[0]?.ch.length ?? 0, h: rows.length, moving: false, animated() { return this.moving; }, rows: (tick) => rows.map((r) => ({ ch: r.ch.map((c, x) => (x === 0 ? String(tick % 10) : c)), st: r.st })) };
  }
  const blank = (w: number, h: number): Row[] => Array.from({ length: h }, () => ({ ch: new Array<string>(w).fill('.'), st: new Array<string>(w).fill('') }));

  test('it runs only while a scene has something that moves and the terminal has the focus', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    const writes: string[] = [];
    const layer = new ArtLayer({ mode: 'truecolor', sink: (s) => writes.push(s), screen: () => ({ columns: 100, rows: 100 }) });
    const scene = still(blank(6, 2));
    const tree = (): React.JSX.Element => (
      <ArtContext.Provider value={layer}>
        <SceneView scene={scene} />
      </ArtContext.Provider>
    );
    const inst = render(tree());
    expect(inst.lastFrame()).toContain('0.....');
    expect(layer.running).toBe(false);
    vi.advanceTimersByTime(1000);
    expect(layer.steps).toBe(0);
    // Something starts to move: the next render starts the clock.
    scene.moving = true;
    inst.rerender(tree());
    await new Promise((r) => setTimeout(r, 20));
    expect(layer.running).toBe(true);
    vi.advanceTimersByTime(1000);
    expect(layer.steps).toBe(8);
    // Each step writes only the cell that changed, and puts the cursor back.
    expect(writes).toHaveLength(8);
    for (const w of writes) {
      expect(w.startsWith('\u001b7')).toBe(true);
      expect(w.endsWith('\u001b8')).toBe(true);
      expect((w.match(/\./g) ?? []).length).toBe(0);
    }
    // In the background it rests, and goes on where it was.
    layer.setFocused(false);
    expect(layer.running).toBe(false);
    const tick = layer.tick;
    vi.advanceTimersByTime(5000);
    expect(layer.tick).toBe(tick);
    layer.setFocused(true);
    vi.advanceTimersByTime(250);
    expect(layer.tick).toBe(tick + 2);
    // The motion is over: the clock stops by itself after the step that sees it.
    scene.moving = false;
    vi.advanceTimersByTime(125);
    expect(layer.running).toBe(false);
    const n = writes.length;
    vi.advanceTimersByTime(5000);
    expect(writes.length).toBe(n);
  });

  test('with the painter: a step is a patch of the changed cells, and no art means no clock', () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout'] });
    const size = { columns: 20, rows: 4 };
    const painter: Painter = new Painter({ write: () => true, size: () => size, overlays: () => layer.overlays(), minFrameMs: 0 });
    const layer: ArtLayer = new ArtLayer({ mode: 'truecolor', sink: null, screen: () => size, painter });
    const scene = still(blank(6, 2));
    scene.moving = true;
    const inst = render(
      <ArtContext.Provider value={layer}>
        <SceneView scene={scene} />
      </ArtContext.Provider>,
    );
    painter.frame(inst.lastFrame() ?? '');
    painter.flush();
    expect(painter.text().slice(0, 2)).toEqual(['0.....', '0.....']);
    const cells = painter.stats.cells;
    vi.advanceTimersByTime(125);
    expect(painter.text().slice(0, 2)).toEqual(['1.....', '1.....']);
    expect(painter.stats.cells - cells).toBe(2);
    expect(layer.steps).toBe(1);
    const none = new ArtLayer({ mode: 'none', sink: () => undefined, screen: () => size });
    render(
      <ArtContext.Provider value={none}>
        <SceneView scene={scene} />
      </ArtContext.Provider>,
    );
    expect(none.running).toBe(false);
  });

  test('the bytes of a step: runs of changed cells, each with one cursor move', () => {
    const a: Row[] = [{ ch: ['a', 'b', 'c', 'd'], st: ['', '', '1', ''] }];
    const b: Row[] = [{ ch: ['a', 'X', 'c', 'Y'], st: ['', '1', '1', ''] }];
    expect(drawRowChanges(a, b, { x: 4, y: 2 })).toBe('\u001b7\u001b[3;6H\u001b[0;1mX\u001b[0m\u001b[3;8HY\u001b[0m\u001b8');
    expect(drawRowChanges(a, a, { x: 0, y: 0 })).toBe('');
    // Nothing known of the screen: every cell.
    expect(drawRowChanges([], a, { x: 0, y: 0 })).toBe('\u001b7\u001b[1;1Hab\u001b[0;1mc\u001b[0md\u001b[0m\u001b8');
  });
});
