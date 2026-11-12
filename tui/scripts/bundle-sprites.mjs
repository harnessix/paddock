#!/usr/bin/env node
// Puts the sprite files that the interface uses into src/sprite-files.ts,
// so tsc puts them in dist and bun build --compile puts them in the single
// binary: sprites/character-COLOR-STATE.json for the 7 colours and the 5
// states, nothing else (not frames.json). The build runs
// this first. It stops on a missing file or a frame count that is not the
// one of sprites/frames.json, so a release never ships a gap.
// Usage: node scripts/bundle-sprites.mjs
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const dir = join(root, 'sprites');
// The same lists as src/sprites.ts (ART_COLORS, STATE_FRAMES); a test checks both.
const COLORS = ['orange', 'blue', 'purple', 'green', 'pink', 'teal', 'gray'];
const FRAMES = { idle: 8, work: 8, wait: 8, sleep: 4, blocked: 4 };

const problems = [];
const entries = [];
for (const color of COLORS) {
  for (const [state, frames] of Object.entries(FRAMES)) {
    const name = `character-${color}-${state}`;
    let sprite;
    try {
      sprite = JSON.parse(readFileSync(join(dir, `${name}.json`), 'utf8'));
    } catch (e) {
      problems.push(`${name}.json: ${e.code === 'ENOENT' ? 'missing' : e.message}`);
      continue;
    }
    const n = Array.isArray(sprite.frames) ? sprite.frames.length : 0;
    if (sprite.w !== 24 || sprite.h !== 24) problems.push(`${name}.json: ${sprite.w} x ${sprite.h} px, not 24 x 24`);
    if (n !== frames) problems.push(`${name}.json: ${n} frames, ${state} has ${frames}`);
    if (sprite.fps !== undefined && sprite.fps !== 8) problems.push(`${name}.json: ${sprite.fps} fps, not 8`);
    // One line of JSON per file: the loader checks the pixels, not this script.
    entries.push(`  ${JSON.stringify(name)}: ${JSON.stringify(JSON.stringify(sprite))},`);
  }
}
if (problems.length > 0) {
  for (const p of problems) console.error(`bundle-sprites: ${p}`);
  process.exit(1);
}
const out = `// Made by scripts/bundle-sprites.mjs from sprites/*.json. Do not edit.
export const SPRITE_FILES: Record<string, string> = {
${entries.join('\n')}
};
`;
writeFileSync(join(root, 'src', 'sprite-files.ts'), out);
