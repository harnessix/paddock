// A thin mouse layer: SGR mouse reports (DECSET 1006) parsed from the
// input, and hit tests against the boxes that Ink laid out. The keyboard
// path never depends on it.

import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from 'react';
import { measureElement, type DOMElement } from 'ink';

export type MouseKind = 'press' | 'release' | 'drag' | 'move' | 'wheelUp' | 'wheelDown';

export interface MouseEvent {
  kind: MouseKind;
  /** 0 left, 1 middle, 2 right, 3 none. */
  button: number;
  /** 0-based column and row. */
  x: number;
  y: number;
  shift: boolean;
}

const SGR = /^(?:\u001b)?\[<(\d+);(\d+);(\d+)([Mm])$/;

/** A mouse report as Ink hands it to useInput (with or without the ESC), or null. */
export function parseMouse(input: string): MouseEvent | null {
  const m = SGR.exec(input);
  if (!m) return null;
  const code = Number(m[1]);
  const x = Number(m[2]) - 1;
  const y = Number(m[3]) - 1;
  if (!Number.isFinite(code) || x < 0 || y < 0) return null;
  // 128 and higher: the extra buttons (back, forward). They never click.
  if (code >= 128) return null;
  const button = code & 3;
  const shift = (code & 4) !== 0;
  let kind: MouseKind;
  if (code & 64) kind = button === 0 ? 'wheelUp' : 'wheelDown';
  else if (code & 32) kind = button === 3 ? 'move' : 'drag';
  else kind = m[4] === 'm' ? 'release' : 'press';
  return { kind, button, x, y, shift };
}

/** Looks like a mouse report, also a broken one: never treat it as keys. */
export function isMouseInput(input: string): boolean {
  return /^(?:\u001b)?\[<[\d;]*[Mm]?$/.test(input);
}

export interface Handlers {
  onClick?: () => void;
  onDoubleClick?: () => void;
  onWheel?: (dir: 1 | -1) => void;
}

interface Target {
  ref: RefObject<DOMElement | null>;
  handlers: RefObject<Handlers>;
  order: number;
}

interface MouseApi {
  enabled: boolean;
  hovered: string | null;
  register: (key: string, t: Target) => () => void;
}

const MouseContext = createContext<MouseApi>({
  enabled: false,
  hovered: null,
  register: () => () => undefined,
});

const DOUBLE_CLICK_MS = 400;

export interface MouseDispatcher {
  dispatch: (e: MouseEvent) => boolean;
}

/** The box under a point: the smallest registered box that contains it. */
function hit(targets: Map<string, Target>, x: number, y: number): [string, Target] | null {
  let best: [string, Target] | null = null;
  let bestArea = Infinity;
  for (const [key, t] of targets) {
    const node = t.ref.current;
    if (!node) continue;
    const m = measureElement(node);
    if (m.width <= 0 || m.height <= 0) continue;
    if (x < m.x || x >= m.x + m.width || y < m.y || y >= m.y + m.height) continue;
    const area = m.width * m.height;
    if (area < bestArea || (area === bestArea && best && t.order > best[1].order)) {
      best = [key, t];
      bestArea = area;
    }
  }
  return best;
}

export function MouseProvider(props: {
  enabled: boolean;
  dispatcherRef: RefObject<MouseDispatcher | null>;
  children: ReactNode;
}): React.JSX.Element {
  const targets = useRef(new Map<string, Target>());
  const counter = useRef(0);
  const [hovered, setHovered] = useState<string | null>(null);
  const lastClick = useRef<{ key: string; at: number } | null>(null);
  const pressed = useRef<string | null>(null);
  const { enabled, dispatcherRef } = props;

  const register = useCallback((key: string, t: Target) => {
    targets.current.set(key, { ...t, order: counter.current++ });
    return () => {
      targets.current.delete(key);
    };
  }, []);

  useEffect(() => {
    dispatcherRef.current = {
      dispatch(e: MouseEvent): boolean {
        if (!enabled) return false;
        const found = hit(targets.current, e.x, e.y);
        if (e.kind === 'move' || e.kind === 'drag') {
          setHovered(found ? found[0] : null);
          return true;
        }
        if (e.kind === 'wheelUp' || e.kind === 'wheelDown') {
          // The pane under the pointer: the innermost box with a wheel handler.
          let best: Target | null = null;
          let bestArea = Infinity;
          for (const t of targets.current.values()) {
            if (!t.handlers.current?.onWheel || !t.ref.current) continue;
            const m = measureElement(t.ref.current);
            if (e.x < m.x || e.x >= m.x + m.width || e.y < m.y || e.y >= m.y + m.height) continue;
            if (m.width * m.height < bestArea) {
              best = t;
              bestArea = m.width * m.height;
            }
          }
          best?.handlers.current?.onWheel?.(e.kind === 'wheelUp' ? -1 : 1);
          return true;
        }
        if (e.button !== 0) return true;
        // A click acts on the release over the box where the press began,
        // so a drag that starts on a button and ends elsewhere does nothing.
        if (e.kind === 'press') {
          pressed.current = found ? found[0] : null;
          return true;
        }
        if (e.kind !== 'release') return true;
        const start = pressed.current;
        pressed.current = null;
        if (!found || start !== found[0]) {
          lastClick.current = null;
          return true;
        }
        const [key, t] = found;
        const at = Date.now();
        const prev = lastClick.current;
        const h = t.handlers.current;
        if (prev && prev.key === key && at - prev.at <= DOUBLE_CLICK_MS && h?.onDoubleClick) {
          lastClick.current = null;
          h.onDoubleClick();
          return true;
        }
        lastClick.current = { key, at };
        h?.onClick?.();
        return true;
      },
    };
    return () => {
      dispatcherRef.current = null;
    };
  }, [enabled, dispatcherRef]);

  const api = useMemo(() => ({ enabled, hovered, register }), [enabled, hovered, register]);
  return <MouseContext.Provider value={api}>{props.children}</MouseContext.Provider>;
}

/** Make a box clickable. Returns whether the pointer is over it. */
export function useClickable(ref: RefObject<DOMElement | null>, handlers: Handlers): boolean {
  const key = useId();
  const api = useContext(MouseContext);
  const h = useRef<Handlers>(handlers);
  h.current = handlers;
  const { register } = api;
  useEffect(() => register(key, { ref, handlers: h, order: 0 }), [register, key, ref]);
  return api.enabled && api.hovered === key;
}

export function useMouseEnabled(): boolean {
  return useContext(MouseContext).enabled;
}
