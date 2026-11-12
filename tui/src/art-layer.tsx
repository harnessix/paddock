// The art layer: draws the sprites and animates them at 8 frames a second.
// Ink draws each sprite in the frame it renders. Between Ink's frames, the
// layer hands the cells of the next frame to the painter of the screen
// (screen.ts), which writes only those that changed. An animation step
// costs no React render and no full redraw. It animates only while the
// terminal has the focus and the sprite is on the screen.

import React, { createContext, useContext, useEffect, useLayoutEffect, useRef, type RefObject } from 'react';
import { Box, Text, measureElement, type DOMElement } from 'ink';
import { drawRuns, prepareSprite, type ArtMode, type Cell, type PreparedSprite } from './art.js';
import { SPRITE_FPS, type Sprite } from './sprites.js';
import type { Overlay } from './screen.js';

export const FPS = SPRITE_FPS;

interface Entry {
  sprite: Sprite;
  ref: RefObject<DOMElement | null>;
  /** The frame in Ink's last render of the sprite. */
  rendered: number;
  /** What the screen shows: the frame and the place, or null (not known). */
  shown: { frame: number; x: number; y: number } | null;
}

export interface ArtLayerOptions {
  mode: ArtMode;
  /** Writes to the terminal, or null: no animation (Ink draws a still frame). */
  sink: ((s: string) => void) | null;
  /** The size of the screen, for "on the screen". */
  screen: () => { columns: number; rows: number };
  fps?: number;
  /**
   * The painter of the screen (screen.ts). With it, the layer hands each
   * step to the painter as cells, and the painter writes those that differ
   * from the screen; `sink` is not used.
   */
  painter?: { patch(o: Overlay): boolean } | null;
  /** The time of one step in ms, for the measure of the animation cost. */
  onStep?: (ms: number) => void;
}

export class ArtLayer {
  readonly mode: ArtMode;
  private sink: ((s: string) => void) | null;
  private screen: () => { columns: number; rows: number };
  private fps: number;
  private painter: { patch(o: Overlay): boolean } | null;
  private onStep: ((ms: number) => void) | undefined;
  private entries = new Set<Entry>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private focused = true;
  /** The frame clock: every sprite shows frame `tick % its frame count`. */
  tick = 0;
  /** Frames drawn by the layer itself (not by Ink): for tests and the curious. */
  steps = 0;

  constructor(o: ArtLayerOptions) {
    this.mode = o.mode;
    this.sink = o.sink;
    this.screen = o.screen;
    this.fps = o.fps ?? FPS;
    this.painter = o.painter ?? null;
    this.onStep = o.onStep;
  }

  /**
   * Whether the painter draws the sprites. Then Ink only keeps their place
   * free: the cells of 20 sprites as text would cost Ink most of the time
   * of a frame, and the painter has them ready.
   */
  get paints(): boolean {
    return this.painter !== null;
  }

  /** Whether the layer can draw a step: it has a painter or a sink. */
  private get canDraw(): boolean {
    return this.painter !== null || this.sink !== null;
  }

  /** The sprites on the screen as cells of the frame of the clock, for the painter. */
  overlays(): Overlay[] {
    if (!this.enabled) return [];
    const out: Overlay[] = [];
    for (const e of this.entries) {
      const at = this.place(e);
      if (at) out.push({ ...at, rows: this.prepared(e.sprite).screen[this.frameOf(e.sprite)] as Overlay['rows'] });
    }
    return out;
  }

  get enabled(): boolean {
    return this.mode !== 'none';
  }

  get hasFocus(): boolean {
    return this.focused;
  }

  get running(): boolean {
    return this.timer !== null;
  }

  frameOf(s: Sprite): number {
    return this.tick % s.frames.length;
  }

  /** A sprite ready for the mode of the layer: made once per sprite (sprites never change). */
  prepared(s: Sprite): PreparedSprite {
    let store = preparedSprites.get(s);
    if (!store) {
      store = new Map<ArtMode, PreparedSprite>();
      preparedSprites.set(s, store);
    }
    let p = store.get(this.mode);
    if (!p) {
      p = prepareSprite(s, this.mode);
      store.set(this.mode, p);
    }
    return p;
  }

  /** The cells of a frame. */
  cellsOf(s: Sprite, frame: number): Cell[][] {
    const cells = this.prepared(s).cells;
    return cells[((frame % cells.length) + cells.length) % cells.length] as Cell[][];
  }

  /** The text of each row of cells of a frame, with its SGR codes. */
  rowsOf(s: Sprite, frame: number): string[] {
    const rows = this.prepared(s).rows;
    return rows[((frame % rows.length) + rows.length) % rows.length] as string[];
  }

  register(e: Entry): () => void {
    this.entries.add(e);
    this.update();
    return () => {
      this.entries.delete(e);
      this.update();
    };
  }

  setFocused(on: boolean): void {
    if (on === this.focused) return;
    this.focused = on;
    this.update();
  }

  /** Start or stop the clock: it runs only when there is something to animate. */
  update(): void {
    const want = this.enabled && this.canDraw && this.focused && [...this.entries].some((e) => e.sprite.frames.length > 1);
    if (want && !this.timer) {
      this.timer = setInterval(() => this.step(), Math.round(1000 / this.fps));
      this.timer.unref?.();
    } else if (!want && this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  /** For good: the layer writes nothing more (the program ends). */
  stop(): void {
    this.sink = null;
    this.painter = null;
    this.entries.clear();
    this.update();
  }

  /** Where a sprite stands on the screen, or null when any part of it is off the screen. */
  private place(e: Entry): { x: number; y: number } | null {
    const node = e.ref.current;
    if (!node) return null;
    const m = measureElement(node);
    const { columns, rows } = this.screen();
    if (m.width <= 0 || m.height <= 0) return null;
    if (m.x < 0 || m.y < 0 || m.x + e.sprite.w > columns || m.y + Math.ceil(e.sprite.h / 2) > rows) return null;
    return { x: m.x, y: m.y };
  }

  /** One animation step: the next frame, only the cells that change, only sprites on the screen. */
  step(): void {
    if (!this.canDraw || !this.focused || !this.enabled) return;
    const t0 = this.onStep ? performance.now() : 0;
    this.tick++;
    if (this.painter) {
      let drew = false;
      for (const o of this.overlays()) if (this.painter.patch(o)) drew = true;
      if (drew) this.steps++;
    } else this.stepSink();
    this.onStep?.(performance.now() - t0);
  }

  private stepSink(): void {
    if (!this.sink) return;
    let out = '';
    for (const e of this.entries) {
      if (e.sprite.frames.length < 2) continue;
      const at = this.place(e);
      if (!at) {
        e.shown = null;
        continue;
      }
      const next = this.frameOf(e.sprite);
      const sameSpot = e.shown && e.shown.x === at.x && e.shown.y === at.y;
      // A sprite not drawn by the layer yet shows Ink's frame.
      const from = sameSpot ? (e.shown as { frame: number }).frame : e.rendered;
      if (from !== next) out += drawRuns(this.prepared(e.sprite).step(from, next), at);
      e.shown = { frame: next, ...at };
    }
    if (out) {
      this.steps++;
      this.sink(out);
    }
  }
}

const preparedSprites = new WeakMap<Sprite, Map<ArtMode, PreparedSprite>>();

export const ArtContext = createContext<ArtLayer>(new ArtLayer({ mode: 'none', sink: null, screen: () => ({ columns: 80, rows: 24 }) }));

export function useArt(): ArtLayer {
  return useContext(ArtContext);
}

/** A sprite in half blocks: w columns, h/2 rows. Nothing at all without art. */
export function SpriteView(props: { sprite: Sprite }): React.JSX.Element | null {
  const layer = useArt();
  const ref = useRef<DOMElement>(null);
  const { sprite } = props;
  const frame = layer.frameOf(sprite);
  const entry = useRef<Entry>({ sprite, ref, rendered: frame, shown: null });
  // The frame of this render is the frame that Ink writes next.
  entry.current.rendered = frame;
  if (entry.current.sprite !== sprite) {
    entry.current.sprite = sprite;
    entry.current.shown = null;
  }
  // In the layout phase: the sprite is known before the painter gets the frame that has its place.
  useLayoutEffect(() => layer.register(entry.current), [layer]);
  // A new sprite (a new state) can have one frame or many: the clock follows.
  useEffect(() => layer.update(), [layer, sprite]);
  if (!layer.enabled) return null;
  if (layer.paints) return <Box ref={ref} width={sprite.w} height={Math.ceil(sprite.h / 2)} flexShrink={0} />;
  const rows = layer.rowsOf(sprite, frame);
  return (
    <Box ref={ref} flexDirection="column" width={sprite.w} height={rows.length} flexShrink={0}>
      {rows.map((r, i) => (
        <Text key={i} wrap="truncate-end">
          {r}
        </Text>
      ))}
    </Box>
  );
}
