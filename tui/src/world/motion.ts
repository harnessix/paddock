// The motion of the world, at the 8 frames a second of the art layer. A
// motion is a short event with a start and a length in frames: a walk to
// the door, frost that grows, a zoom. Time is the frame clock of the art
// layer, so a motion rests while the terminal is in the background and
// goes on where it was.
//
// Reduce Motion: nothing travels. A change that the design names an
// instant swap is over when it starts (the walk to the door, the window,
// the frost, the climb). A change of the whole picture or a thing that
// appears is a cross-fade of 150 ms. A call of a tool is a cable that is
// bright for one second, and a written note is a dot for two seconds.
//
// Still: the terminal has no art or no way to draw a step (NO_COLOR,
// --plain). No clock runs, so no event is kept at all.

/** The lengths of the motions of the design, in frames of 125 ms. */
export const FRAMES = {
  /** The character walks to the door with its card: 500 ms. */
  ask: 4,
  /** It walks back after the answer: 400 ms. */
  answer: 3,
  /** Frost grows from the corners, and melts: 600 ms. */
  freeze: 5,
  thaw: 5,
  /** The window changes its mode: 300 ms. */
  net: 3,
  /** The flag of the mailbox goes up: 200 ms. */
  mail: 2,
  /** A helper's room builds at the end of its path: 800 ms. */
  spawn: 6,
  /** The envelope of an ended helper travels to its parent: 500 ms. */
  end: 4,
  /** The fence of a yard extends to a new room: 500 ms. */
  join: 4,
  /** A stool of the build bench gets its holder: 250 ms. */
  slot: 2,
  /** A new notebook slides onto the shelf: 250 ms. */
  note: 2,
  /** The walls turn to glass: 300 ms. */
  plumb: 3,
  /** The camera zooms on a building or a room: 450 ms. */
  zoom: 4,
  /** The camera goes up to the loft: 350 ms. */
  pan: 3,
  /** The walls of glass turn back to walls: 300 ms. */
  unplumb: 3,
  /** The mailbox hops once for a question: 300 ms. */
  hop: 3,
  /** The character climbs the ladder to the loft, and comes down again: 600 ms. */
  climb: 5,
  descend: 5,
  /** It writes a note upstairs: one loop of its work, 1 s. */
  write: 8,
  /** A call of a tool: a spark along its cable, 400 ms. */
  spark: 3,
  /** The card that rises at a socket with a cover: 250 ms. */
  rise: 2,
  /** A tag of the shared notes flips: 200 ms. */
  flip: 2,
  /** After an apply, the changed parts move from the desk into the house: 1 s. */
  apply: 8,
  /** The first look at the world: the door, the window and the ladder pulse once. */
  pulse: 8,
  /** Reduce Motion: the cross-fade of every transition, 150 ms. */
  fade: 1,
  /** Reduce Motion: the cable of a call is bright for 1 s. */
  glow: 8,
  /** Reduce Motion: a dot beside a note that was written, 2 s. */
  dot: 16,
} as const;

/** The fan of another computer turns once in 2 s (16 frames) while a job runs there. */
export const FAN_LOOP = 16;

/** How long a character stays upstairs, from the first step on the ladder to the last. */
export const LOFT_VISIT = FRAMES.climb + FRAMES.write + FRAMES.descend;

export type MotionKind = keyof typeof FRAMES;

/**
 * What a motion is with Reduce Motion. A kind that is not here is an
 * instant swap: it is over when it starts.
 */
export const REDUCED: Partial<Record<MotionKind, MotionKind>> = {
  // The camera, and the walls of glass: a cross-fade of the picture.
  zoom: 'fade',
  pan: 'fade',
  plumb: 'fade',
  unplumb: 'fade',
  // A thing that appears: it fades in.
  join: 'fade',
  spawn: 'fade',
  // A call: the cable is bright for a second. A note: a dot for two seconds.
  spark: 'glow',
  flip: 'dot',
  // The character appears upstairs, and is back when it wrote.
  write: 'write',
};

interface Event {
  at: number;
  frames: number;
  /** The form of Reduce Motion: the thing shows its end state, with a sign of the change. */
  reduced: boolean;
  /** What the thing was before (the old mode of a window). */
  from?: string;
}

/** Where the character of a room is on its way to the loft. */
export interface LoftTrip {
  /** In its room, on the ladder, or upstairs. */
  where: 'room' | 'ladder' | 'loft';
  /** On the ladder: 0 at its foot, 1 at its top. */
  p: number;
}

export class Motion {
  /** Reduce Motion: no walk, no growth, no zoom. See REDUCED. */
  reduce: boolean;
  /** No clock at all (no art, or no way to draw a step): every change shows at once. */
  still = false;
  private events = new Map<string, Event>();
  /** The envelopes of helpers that ended, by helper: to whom each travels. */
  private envelopes = new Map<string, { parent: string; at: number }>();

  constructor(reduce = false) {
    this.reduce = reduce;
  }

  /** Start a motion of a thing (a run, a yard) at a frame of the clock. `from` is what the thing was before. */
  start(kind: MotionKind, id: string, tick: number, from?: string): void {
    if (this.still) return;
    if (!this.reduce) {
      this.events.set(`${kind}:${id}`, { at: tick, frames: FRAMES[kind], reduced: false, from });
      return;
    }
    const as = REDUCED[kind];
    if (!as) return;
    // A state that lasts (the character upstairs) is the same with Reduce Motion. The rest is a sign of the change.
    this.events.set(`${kind}:${id}`, { at: tick, frames: FRAMES[as], reduced: as !== kind, from });
  }

  /** How far a motion is: 0 at its start, 1 when it is over or never began. */
  progress(kind: MotionKind, id: string, tick: number): number {
    const e = this.events.get(`${kind}:${id}`);
    // A motion that waits for its frame has not begun: the thing is at rest.
    if (!e || e.reduced || tick < e.at) return 1;
    const t = (tick - e.at) / e.frames;
    return t >= 1 ? 1 : t;
  }

  /** Whether a motion of a thing runs at this frame. */
  running(kind: MotionKind, id: string, tick: number): boolean {
    const e = this.events.get(`${kind}:${id}`);
    // A motion can start a few frames from now (after a zoom): it waits until then.
    return e !== undefined && !e.reduced && tick >= e.at && tick - e.at < e.frames;
  }

  /** What the thing was before its motion began, while the motion is known. */
  from(kind: MotionKind, id: string): string | undefined {
    return this.events.get(`${kind}:${id}`)?.from;
  }

  /**
   * Reduce Motion: whether the sign of a change shows at this frame (the
   * bright cable of a call, the dot of a note, the frame of a cross-fade).
   */
  hint(kind: MotionKind, id: string, tick: number): boolean {
    const e = this.events.get(`${kind}:${id}`);
    return e !== undefined && e.reduced && tick >= e.at && tick - e.at < e.frames;
  }

  /**
   * Reduce Motion: how much of a thing that fades in shows. 1 when
   * nothing fades. A fade of one frame shows the thing half.
   */
  fade(kind: MotionKind, id: string, tick: number): number {
    const e = this.events.get(`${kind}:${id}`);
    if (!e || !e.reduced || tick < e.at || tick - e.at >= e.frames) return 1;
    return (tick - e.at + 1) / (e.frames + 1);
  }

  /** A helper ended: its envelope travels the path to the run that started it. */
  end(child: string, parent: string, tick: number): void {
    if (this.reduce || this.still) return;
    this.envelopes.set(child, { parent, at: tick });
  }

  /** How far each envelope on its way to a run is: 0 at the helper, 1 at the door. */
  ended(parent: string, tick: number): number[] {
    const out: number[] = [];
    for (const e of this.envelopes.values()) {
      const t = (tick - e.at) / FRAMES.end;
      if (e.parent === parent && t >= 0 && t < 1) out.push(t);
    }
    return out;
  }

  /**
   * A note was written in the loft of a house. With a room, its character
   * climbs the ladder, writes, and comes down; the new notebook slides
   * onto the shelf while it writes. Without one, only the notebook moves.
   */
  wrote(house: string, room: string | null, tick: number): void {
    if (!room) {
      this.start('note', house, tick);
      return;
    }
    if (this.reduce) {
      // The character appears upstairs, and the notebook is on the shelf.
      this.start('write', room, tick);
      return;
    }
    this.start('climb', room, tick);
    this.start('write', room, tick + FRAMES.climb);
    this.start('note', house, tick + FRAMES.climb + FRAMES.write - FRAMES.note);
    this.start('descend', room, tick + FRAMES.climb + FRAMES.write);
  }

  /** Where the character of a room is at this frame: in its room, on the ladder, or in the loft. */
  trip(room: string, tick: number): LoftTrip {
    // The middle of each frame: the first frame is already a step up, the last one a step from the floor.
    const half = 1 / (2 * FRAMES.climb);
    if (this.running('climb', room, tick)) return { where: 'ladder', p: this.progress('climb', room, tick) + half };
    if (this.running('write', room, tick)) return { where: 'loft', p: 1 };
    if (this.running('descend', room, tick)) return { where: 'ladder', p: 1 - this.progress('descend', room, tick) - half };
    return { where: 'room', p: 0 };
  }

  /**
   * A tool was called from a room: a spark runs along its cable. A tool
   * that asks has a cover on its socket: the spark stops there and a card
   * rises.
   */
  called(room: string, tool: string, ask: boolean, tick: number): void {
    const id = `${room}:${tool}`;
    this.start('spark', id, tick);
    if (ask) this.start('rise', id, tick + FRAMES.spark);
  }

  /** Whether any motion runs at this frame, or waits to start. Events that are over are dropped. */
  active(tick: number): boolean {
    let any = false;
    for (const [k, e] of this.envelopes) {
      if (tick - e.at < FRAMES.end) any = true;
      else this.envelopes.delete(k);
    }
    for (const [k, e] of this.events) {
      if (tick - e.at < e.frames) any = true;
      else this.events.delete(k);
    }
    return any;
  }
}

/** Ease out: fast at first, then slow, as a spring that settles. */
export function ease(t: number): number {
  const k = Math.max(0, Math.min(1, t));
  return 1 - (1 - k) * (1 - k);
}

/** A whole number between a and b at progress t. */
export function lerp(a: number, b: number, t: number): number {
  return Math.round(a + (b - a) * Math.max(0, Math.min(1, t)));
}

/**
 * A spring that settles (critically damped): fast at first, then it
 * comes to rest without a bounce. The zoom of the camera.
 */
export function spring(t: number): number {
  const k = Math.max(0, Math.min(1, t));
  if (k >= 1) return 1;
  const at = (x: number): number => 1 - (1 + 6 * x) * Math.exp(-6 * x);
  return at(k) / at(1);
}

/** Up and down once: 0 at both ends, 1 in the middle. A hop. */
export function arc(t: number): number {
  const k = Math.max(0, Math.min(1, t));
  return 4 * k * (1 - k);
}
