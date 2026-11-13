// The world on the screen: the scene of the view (ranch, building, room
// or loft) with the Inspector over it, the zoom between the stops of the
// camera, and the places that a click selects.

import React, { useMemo, useRef, type MutableRefObject } from 'react';
import { Box, type DOMElement } from 'ink';
import type { ArtMode } from '../art.js';
import { charCells } from '../clean.js';
import { SceneView, useArt, type Scene } from '../art-layer.js';
import { usePointer } from '../mouse.js';
import type { Row } from '../screen.js';
import { useTheme } from '../theme.js';
import { drawCard, type CardView } from '../world/card.js';
import { drawInspector, inspect } from '../world/inspect.js';
import { now } from '../model.js';
import type { World } from '../world/model.js';
import { crossfade } from '../world/fade.js';
import { ease, lerp, spring, type Motion } from '../world/motion.js';
import { drawWorld, roomPanelX, type Hit, type View } from '../world/scene.js';

/** A rectangle of cells. */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** The scene of a view, with the things that can be selected and the one that is. */
export interface WorldScene extends Scene {
  hits: Hit[];
  /** The selection that the scene shows: the one of the view, or a default when that is not in the picture. */
  sel: string;
  /** The rows that the scene drew last, or none: what was on the screen before the camera moves. */
  last: () => Row[];
}

export interface WorldSceneOptions {
  w: number;
  h: number;
  mode: ArtMode;
  plain?: boolean;
  /** A light terminal. */
  light?: boolean;
  motion: Motion;
  inspector?: boolean;
  /** The request card that is open at the door of the room in view. */
  card?: CardView | null;
  readonly?: boolean;
  /** Where the zoom into this view began: the thing that was opened. */
  zoomFrom?: Rect | null;
  /** The picture before the camera moved or the walls turned to glass. The new one opens over it, or fades in from it. */
  before?: readonly Row[] | null;
  /** Unix seconds, for the words of time. */
  t?: number;
  /** The frame of the clock at which the scene is made. */
  tick?: number;
}

/** What to select when the selection of the view is not in the picture. */
export function defaultSelection(world: World, view: View, hits: Hit[]): string {
  if (hits.some((h) => h.id === view.sel)) return view.sel;
  if (view.level === 'room') {
    const asks = world.houses.flatMap((h) => h.rooms).find((r) => r.id === view.room)?.asks.length ?? 0;
    const want = asks > 0 ? 'door' : 'self';
    return hits.find((h) => h.id === want)?.id ?? hits[0]?.id ?? '';
  }
  const rooms = hits.filter((h) => h.kind === 'room');
  const needs = rooms.find((h) => (world.houses.flatMap((x) => x.rooms).find((r) => r.id === h.room)?.asks.length ?? 0) > 0);
  return (needs ?? rooms[0] ?? hits[0])?.id ?? '';
}

function blank(row: Row, keep: (x: number) => boolean): Row {
  const out: Row = { ch: row.ch.slice(), st: row.st.slice() };
  for (let x = 0; x < out.ch.length; x++) {
    if (keep(x)) continue;
    out.ch[x] = ' ';
    out.st[x] = '';
  }
  // A wide character that the edge cut: a blank.
  for (let x = 0; x < out.ch.length; x++) if (out.ch[x] === '' && (x === 0 || out.ch[x - 1] === ' ')) out.ch[x] = ' ';
  return out;
}

/** A row of the old picture, or a blank one when there is none. */
function old(before: readonly Row[] | null, y: number, like: Row): Row {
  return before?.[y] ?? blank(like, () => false);
}

/** A row with the cells of `inside` in a span of columns, and the cells of `outside` in the rest. */
function span(inside: Row, outside: Row, x0: number, x1: number): Row {
  const out: Row = { ch: outside.ch.slice(), st: outside.st.slice() };
  for (let x = Math.max(0, x0); x < Math.min(out.ch.length, x1); x++) {
    out.ch[x] = inside.ch[x] as string;
    out.st[x] = inside.st[x] as string;
  }
  // A wide character that an edge of the span cut: a blank.
  for (const e of [x0, x1]) {
    if (e <= 0 || e >= out.ch.length) continue;
    if (out.ch[e] === '') out.ch[e] = ' ';
    if (charCells(out.ch[e - 1] as string) === 2) out.ch[e - 1] = ' ';
  }
  return out;
}

/**
 * The zoom in: the picture opens from the thing that was opened to the
 * whole view, over the picture that was there. The zoom out is the same
 * in reverse: the old picture closes on the thing that it was.
 */
function reveal(inside: Row[], outside: readonly Row[] | null, from: Rect, w: number, h: number, p: number): Row[] {
  const x0 = lerp(from.x, 0, p);
  const y0 = lerp(from.y, 0, p);
  const x1 = lerp(from.x + from.w, w, p);
  const y1 = lerp(from.y + from.h, h, p);
  return inside.map((r, y) => {
    if (y === 0) return r;
    const out = old(outside, y, r);
    return y >= y0 && y < y1 ? span(r, out, x0, x1) : out;
  });
}

/**
 * The pan up to the loft: the camera goes up, so the new picture comes
 * down from the top and the old one leaves at the bottom. The pan down
 * from the loft is the reverse.
 */
function pan(rows: Row[], before: readonly Row[] | null, h: number, p: number, down: boolean): Row[] {
  const H = h - 1;
  const d = lerp(0, H, p);
  if (d >= H) return rows;
  return rows.map((r, y) => {
    if (y === 0) return r;
    if (down) return y > H - d ? (rows[y - (H - d)] as Row) : y + d <= H ? old(before, y + d, r) : blank(r, () => false);
    return y <= d ? (rows[y + H - d] as Row) : old(before, y - d, r);
  });
}

/** The transitions that are a cross-fade with Reduce Motion. */
const FADES = ['zoom', 'pan', 'plumb', 'unplumb'] as const;

export function worldScene(world: World, view: View, o: WorldSceneOptions): WorldScene {
  const base = { w: o.w, h: o.h, mode: o.mode, plain: o.plain, light: o.light, motion: o.motion, t: o.t };
  // The first draw finds the things of the picture; the selection must be one of them.
  const first = drawWorld(world, { ...base, tick: o.tick ?? 0, view });
  // The picture before the camera moved: only one of this size can show under the new one.
  const before = o.before && o.before.length === o.h && o.before[0]?.ch.length === o.w ? o.before : null;
  const sel = defaultSelection(world, view, first.hits);
  const shown: View = sel === view.sel ? view : { ...view, sel };
  // The box beside the picture: the request card, or the Inspector. In the room it takes the place of the words.
  // With art the room stands in the middle of the picture: the box begins where its words begin.
  const boxX = view.level === 'room' ? Math.min(roomPanelX(world, view, o) - (o.mode === 'none' ? 3 : 2), Math.max(0, o.w - 30)) : Math.max(0, o.w - Math.min(44, Math.floor(o.w / 2)));
  const boxW = Math.min(o.w - boxX, 64);
  const hits = first.hits.slice();
  let at = -1;
  let rows: Row[] = [];
  let loops = first.loops;
  const draw = (tick: number): Row[] => {
    if (tick === at) return rows;
    const d = drawWorld(world, { ...base, tick, view: shown });
    loops = d.loops;
    if (o.card) {
      const choices = drawCard(d.canvas, o.card, boxX, 1, boxW, o.h - 2, o.t ?? now());
      // The answers are things to click. They are found once: the card does not move.
      if (!hits.some((h) => h.id.startsWith('choice:'))) hits.push(...choices);
    } else if (o.inspector) {
      drawInspector(d.canvas, inspect(world, d.hits.find((h) => h.id === sel), o.readonly), boxX, 1, boxW, o.h - 2);
    }
    let out = d.canvas.rows();
    const m = o.motion;
    const middle: Rect = { x: Math.floor(o.w / 4), y: Math.floor(o.h / 4), w: Math.floor(o.w / 2), h: Math.floor(o.h / 2) };
    if (m.running('zoom', 'world', tick)) {
      // The camera is a spring: fast at first, then it settles.
      const p = spring(m.progress('zoom', 'world', tick));
      if (m.from('zoom', 'world') !== 'out') out = reveal(out, before, o.zoomFrom ?? middle, o.w, o.h, p);
      else if (before) {
        // One stop up: the old picture closes on the thing that it was in the new one.
        const to = d.hits.find((h) => h.id === sel);
        out = reveal(before as Row[], out, to ? { x: to.x, y: to.y, w: to.w, h: to.h } : middle, o.w, o.h, 1 - p).map((r, y) => (y === 0 ? (out[0] as Row) : r));
      }
    } else if (m.running('pan', 'world', tick)) out = pan(out, before, o.h, ease(m.progress('pan', 'world', tick)), m.from('pan', 'world') === 'down');
    else if (before) {
      // Reduce Motion: every transition is a cross-fade of 150 ms.
      const shown = Math.min(...FADES.map((k) => m.fade(k, 'world', tick)));
      if (shown < 1) out = crossfade(before, out, shown);
    }
    at = tick;
    rows = out;
    return rows;
  };
  // The card is drawn now, so its answers are known before the first click.
  if (o.card) draw(o.tick ?? 0);
  return {
    w: o.w,
    h: o.h,
    hits,
    sel,
    last: () => rows,
    rows: draw,
    animated: (tick) => {
      draw(tick);
      return loops || o.motion.active(tick);
    },
  };
}

/** What the interface needs of the scene for its keys: the things, and the selection. */
export interface WorldHandle {
  hits: Hit[];
  sel: string;
}

function HitBox(props: { hit: Hit; onClick: () => void; onDoubleClick: () => void }): React.JSX.Element {
  const ref = useRef<DOMElement>(null);
  usePointer(ref, { onClick: props.onClick, onDoubleClick: props.onDoubleClick });
  const h = props.hit;
  return <Box ref={ref} position="absolute" marginLeft={h.x} marginTop={h.y} width={h.w} height={h.h} />;
}

export function WorldPane(props: {
  world: World;
  view: View;
  width: number;
  height: number;
  motion: Motion;
  inspector: boolean;
  /** The request card that is open, or null. */
  card: CardView | null;
  readonly: boolean;
  zoomFrom: Rect | null;
  /** Unix seconds: the words of time in the picture. */
  t: number;
  handle: MutableRefObject<WorldHandle>;
  onSelect: (id: string) => void;
  onOpen: (id: string) => void;
}): React.JSX.Element {
  const art = useArt();
  const theme = useTheme();
  const { world, view, width, height, motion, inspector, card, readonly, zoomFrom, t } = props;
  // What was on the screen before the camera moved or the walls turned to glass: the next picture opens over it.
  const shown = useRef<{ scene: WorldScene; stop: string } | null>(null);
  const before = useRef<Row[] | null>(null);
  const stop = `${view.level}\n${view.house}\n${view.room}\n${view.plumbing}`;
  const scene = useMemo(
    () => {
      const was = shown.current;
      if (was && was.stop !== stop) before.current = was.scene.last();
      const next = worldScene(world, view, { w: width, h: height, mode: theme.plain ? 'none' : art.mode, plain: theme.plain, light: theme.name === 'light', motion, inspector, card, readonly, zoomFrom, before: before.current, t, tick: art.tick });
      shown.current = { scene: next, stop };
      return next;
    },
    // The card is a new object at each render of the interface: its parts say whether it changed.
    [world, view, width, height, art, theme.plain, theme.name, motion, inspector, card?.pending, card?.state.focus, card?.state.armed?.key, card?.state.typing, readonly, zoomFrom, t],
  );
  props.handle.current = { hits: scene.hits, sel: scene.sel };
  return (
    <Box width={width} height={height} flexDirection="column" overflow="hidden">
      <SceneView scene={scene} />
      {scene.hits.map((h) => (
        <HitBox key={h.id} hit={h} onClick={() => props.onSelect(h.id)} onDoubleClick={() => props.onOpen(h.id)} />
      ))}
    </Box>
  );
}
