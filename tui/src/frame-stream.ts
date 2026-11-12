// The stream that Ink writes to when the painter draws the screen. Ink (in
// its debug mode) writes each frame whole, as text with colour codes. The
// stream hands the frames to the painter and passes the size and the
// resize events of the terminal on to Ink.

import { Writable } from 'node:stream';
import type { Painter } from './screen.js';

/** Only mode switches such as "show the cursor": they go to the terminal as they are. */
const MODES = /^(?:\u001b\[\?[\d;]+[hl])+$/;

export function frameStream(out: NodeJS.WriteStream, painter: Painter, hostWrite: (s: string) => boolean): NodeJS.WriteStream {
  const stream = new Writable({
    decodeStrings: false,
    write(chunk: string | Buffer, _enc, done) {
      const text = typeof chunk === 'string' ? chunk : chunk.toString('utf8');
      if (MODES.test(text)) hostWrite(text);
      // Ink writes the frame in the middle of React's commit. One microtask
      // later the commit is done, and the art layer knows every sprite of the frame.
      else if (text !== '') queueMicrotask(() => painter.frame(text));
      done();
    },
  });
  Object.defineProperties(stream, {
    isTTY: { value: true },
    columns: { get: () => out.columns || 80 },
    rows: { get: () => out.rows || 24 },
  });
  out.on('resize', () => stream.emit('resize'));
  return stream as unknown as NodeJS.WriteStream;
}
