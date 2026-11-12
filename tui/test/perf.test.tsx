// The cost of the interface with 20 rooms on a 200 x 60 screen, measured
// with a fake clock: what one frame costs, what the animation costs, and
// that nothing at all happens while nothing changes.
// PADDOCK_TUI_STATS=1 prints the numbers.
import React, { Profiler } from 'react';
import { afterEach, expect, test, vi } from 'vitest';
import { cleanup, render } from 'ink-testing-library';
import { App } from '../src/app.js';
import { paddock, type PaddockApi } from '../src/paddock.js';
import { makeTheme, ThemeContext } from '../src/theme.js';
import { ArtContext, ArtLayer } from '../src/art-layer.js';
import { Painter } from '../src/screen.js';
import type { Ps } from '../src/types.js';

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

const COLUMNS = 200;
const ROWS = 60;
// The fake clock starts at the real time, on a whole second.
const START = Math.floor(Date.now() / 1000) * 1000;
const realTimeout = setTimeout;
/** Let the promises and React's own scheduler run, in real time. */
const breathe = () => new Promise<void>((r) => realTimeout(r, 0));

function world(): { api: PaddockApi; ps: Ps; reads: () => number } {
  const harnesses = ['claude', 'codex', 'gemini', 'copilot', 'grok', 'opencode', 'aider'];
  const now = START / 1000;
  const ps: Ps = {
    api_version: 'paddock.ps/v1',
    sandboxes: Array.from({ length: 20 }, (_, i) => ({
      id: `${harnesses[i % 7]}-proj${i}-${String(i).padStart(2, '0')}aabbcc`,
      short: `${String(i).padStart(2, '0')}aabb`,
      harness: harnesses[i % 7],
      project: `/src/proj${i}`,
      backend: 'bwrap',
      network: 'egress',
      // Hours ago plus 100 s: no word of time changes in the minutes of the test.
      started: now - 3600 * (1 + (i % 5)) - 100,
      activity: `cargo test --lib mod${i}::`,
      ...(i % 5 === 3 ? { idle_since: now - 3700 } : {}),
    })),
    ended: [],
  };
  let reads = 0;
  const api: PaddockApi = {
    ...paddock,
    ps: async () => (reads++, JSON.parse(JSON.stringify(ps)) as Ps),
    approvals: async () => (reads++, { pending: [] }),
    board: async () => (reads++, { projects: [] }),
    mailBoxes: async () => (reads++, []),
    hasSplit: async () => false,
    harnesses: async () => [],
  };
  return { api, ps, reads: () => reads };
}

function median(v: number[]): number {
  const s = [...v].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)] ?? 0;
}

test('20 rooms: a frame is fast, a step of the animation is cheap, and nothing happens while nothing changes', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'], now: START });
  const w = world();
  const commits: number[] = [];
  const stepMs: number[] = [];
  const size = { columns: COLUMNS, rows: ROWS };
  let written = 0;
  const painter: Painter = new Painter({ write: (s) => ((written += s.length), true), size: () => size, overlays: () => layer.overlays(), minFrameMs: 0 });
  const layer: ArtLayer = new ArtLayer({ mode: 'truecolor', sink: null, screen: () => size, painter, onStep: (ms) => stepMs.push(ms) });
  const inst = render(
    <Profiler id="app" onRender={(_id, _phase, actual) => commits.push(actual)}>
      <ThemeContext.Provider value={makeTheme('dark')}>
        <ArtContext.Provider value={layer}>
          <App api={w.api} initialMouse intervalMs={2000} size={size} />
        </ArtContext.Provider>
      </ThemeContext.Provider>
    </Profiler>,
  );
  // Ink asks the terminal for its size (it runs tput) when the stream has none.
  Object.defineProperty(inst.stdout, 'columns', { value: COLUMNS, configurable: true });
  Object.defineProperty(inst.stdout, 'rows', { value: ROWS, configurable: true });
  const frame = () => inst.lastFrame() ?? '';
  const inkWrite = inst.stdout.write;
  let inkFrames = 0;
  const paintMs: number[] = [];
  inst.stdout.write = (f: string) => {
    inkFrames++;
    inkWrite(f);
    // As in the program (frame-stream.ts): after React's commit.
    queueMicrotask(() => {
      const t0 = performance.now();
      painter.frame(f);
      paintMs.push(performance.now() - t0);
    });
  };
  /** Let `ms` of the fake clock pass, in steps, with room for promises and React in between. */
  const pass = async (ms: number, step = 125): Promise<void> => {
    for (let t = 0; t < ms; t += step) {
      await vi.advanceTimersByTimeAsync(step);
      await breathe();
    }
  };
  for (let i = 0; i < 40 && !(frame().includes('1/20') && layer.overlays().length > 0); i++) await pass(250);
  expect(frame()).toContain('Runs · 20 live, 0 ended · 1/20');
  // 7 cards in a row, 3 rows of cards on the screen, 2 rows of text over them.
  expect(layer.overlays().length).toBe(20);
  await pass(1000);

  // 1. Nothing changes and the terminal is in the background: a whole minute of nothing.
  layer.setFocused(false);
  await pass(500);
  const idle = { commits: commits.length, ink: inkFrames, bytes: written, steps: layer.steps, reads: w.reads(), cpu: process.cpuUsage() };
  await pass(60_000, 500);
  const idleCpu = process.cpuUsage(idle.cpu);
  const idleCpuMs = (idleCpu.user + idleCpu.system) / 1000;
  // paddock was read all the time: the interface saw that nothing changed.
  expect(w.reads() - idle.reads).toBeGreaterThan(30);
  expect(commits.length - idle.commits).toBe(0);
  expect(inkFrames - idle.ink).toBe(0);
  expect(written - idle.bytes).toBe(0);
  expect(layer.steps - idle.steps).toBe(0);
  expect(layer.running).toBe(false);
  // Under 1 % of one processor for the minute (most of it is the test's own clock).
  expect(idleCpuMs).toBeLessThan(600);

  // 2. With the focus: 8 steps a second, each a patch of changed cells, and no React render.
  layer.setFocused(true);
  stepMs.length = 0;
  const anim = { commits: commits.length, ink: inkFrames, bytes: written, steps: layer.steps, cells: painter.stats.cells };
  await pass(10_000);
  const steps = layer.steps - anim.steps;
  expect(steps).toBeGreaterThanOrEqual(79);
  expect(steps).toBeLessThanOrEqual(81);
  expect(commits.length - anim.commits).toBe(0);
  expect(inkFrames - anim.ink).toBe(0);
  const cellsPerStep = (painter.stats.cells - anim.cells) / steps;
  // Far fewer cells than the 20 sprites have (20 x 24 x 12 = 5760), let alone the screen (12 000).
  expect(cellsPerStep).toBeLessThan(5760 / 2);
  expect(median(stepMs)).toBeLessThan(10);

  // 3. A key: the new frame goes to the terminal in the same turn of the event loop.
  const keyMs: number[] = [];
  const renderMs: number[] = [];
  for (let i = 0; i < 12; i++) {
    const before = { ink: inkFrames, commits: commits.length, painted: painter.stats.painted };
    // Processor time, not wall time: other tests run at the same time.
    const t0 = process.cpuUsage();
    inst.stdin.write(i % 2 === 0 ? '\u001b[C' : '\u001b[D');
    let turns = 0;
    for (; turns < 50 && painter.stats.painted === before.painted; turns++) await Promise.resolve();
    const used = process.cpuUsage(t0);
    keyMs.push((used.user + used.system) / 1000);
    expect(inkFrames - before.ink, 'a frame for the key, before any timer').toBeGreaterThanOrEqual(1);
    expect(painter.stats.painted - before.painted).toBe(1);
    renderMs.push(...commits.slice(before.commits));
    await breathe();
  }
  expect(frame()).toContain('1/20');
  // One frame of 200 x 60 with 20 rooms, from the key to the bytes for the terminal: its processor time.
  // About 20 ms on a workstation, about 90 ms on a shared CI runner. The bound catches a real slowdown
  // (a full redraw per key, a timer before the frame), not the speed of the machine.
  expect(median(keyMs)).toBeLessThan(250);

  // 4. A change in paddock: one frame, and only the cells of the changed card.
  const sandbox = w.ps.sandboxes[0];
  if (sandbox) sandbox.activity = 'cargo build: done';
  const change = { ink: inkFrames, cells: painter.stats.cells, steps: layer.steps };
  layer.setFocused(false);
  await pass(10_000, 250);
  expect(frame()).toContain('cargo build: done');
  expect(inkFrames - change.ink).toBe(1);
  expect(painter.stats.cells - change.cells).toBeLessThan(40);
  expect(painter.stats.clears).toBe(1);

  if (process.env.PADDOCK_TUI_STATS) {
    process.stderr.write(
      `\n20 rooms, ${COLUMNS}x${ROWS}: idle minute ${idleCpuMs.toFixed(0)} ms CPU, 0 renders, 0 bytes | ` +
        `animation step median ${median(stepMs).toFixed(2)} ms, max ${Math.max(...stepMs).toFixed(2)} ms, ${cellsPerStep.toFixed(0)} cells, ${((written - anim.bytes) / steps / 1024).toFixed(1)} kB | ` +
        `key to frame median ${median(keyMs).toFixed(1)} ms, max ${Math.max(...keyMs).toFixed(1)} ms (React ${median(renderMs).toFixed(1)} ms, painter ${median(paintMs.slice(-12)).toFixed(1)} ms)\n`,
    );
  }
});
