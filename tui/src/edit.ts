import type { Key } from 'ink';
import { oneLine } from './clean.js';

/** One key on a one-line text field: the new value, or null when the key is not for the field. */
export function editLine(value: string, input: string, key: Key): string | null {
  if (key.backspace || key.delete) return Array.from(value).slice(0, -1).join('');
  if (key.ctrl && input === 'u') return '';
  if (key.ctrl || key.meta || key.escape || key.return || key.tab) return null;
  if (key.upArrow || key.downArrow || key.leftArrow || key.rightArrow || key.pageUp || key.pageDown) return null;
  // Typed or pasted text: no control characters, one line.
  const add = oneLine(input.replace(/[\r\n]+/g, ' '));
  if (input.length > 0 && add.length === 0 && input !== ' ') return value;
  return value + (input === ' ' ? ' ' : add);
}
