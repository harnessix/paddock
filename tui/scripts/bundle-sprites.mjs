#!/usr/bin/env node
// Puts the sprite files that the interface uses into src/sprite-files.ts,
// so tsc puts them in dist and bun build --compile puts them in the single
// binary: sprites/character-COLOR-STATE.json for the 7 colours and the 5
// states, and the layers of the world, sprites/world/NAME.json for each
// name in sprites/world/frames.json. The build runs this first. It stops
// on a missing file, or on a size or a frame count that is not the one of
// frames.json, so a release never ships a gap.
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
// The layers of the world: the list is sprites/world/frames.json.
const worldDir = join(dir, 'world');
const world = [];
let meta = {};
try {
  meta = JSON.parse(readFileSync(join(worldDir, 'frames.json'), 'utf8'));
} catch (e) {
  problems.push(`world/frames.json: ${e.code === 'ENOENT' ? 'missing' : e.message}`);
}
for (const [name, want] of Object.entries(meta)) {
  let sprite;
  try {
    sprite = JSON.parse(readFileSync(join(worldDir, `${name}.json`), 'utf8'));
  } catch (e) {
    problems.push(`world/${name}.json: ${e.code === 'ENOENT' ? 'missing' : e.message}`);
    continue;
  }
  const n = Array.isArray(sprite.frames) ? sprite.frames.length : 0;
  if (sprite.w !== want.w || sprite.h !== want.h) problems.push(`world/${name}.json: ${sprite.w} x ${sprite.h} px, not ${want.w} x ${want.h}`);
  if (n !== want.frames) problems.push(`world/${name}.json: ${n} frames, not ${want.frames}`);
  if (sprite.fps !== want.fps) problems.push(`world/${name}.json: ${sprite.fps} fps, not ${want.fps}`);
  world.push(`  ${JSON.stringify(name)}: ${JSON.stringify(JSON.stringify(sprite))},`);
}
if (problems.length > 0) {
  for (const p of problems) console.error(`bundle-sprites: ${p}`);
  process.exit(1);
}
const out = `// Made by scripts/bundle-sprites.mjs from sprites/*.json and sprites/world/*.json. Do not edit.
export const SPRITE_FILES: Record<string, string> = {
${entries.join('\n')}
};

/** The layers of the world, from sprites/world/*.json. */
export const WORLD_FILES: Record<string, string> = {
${world.join('\n')}
};
`;
writeFileSync(join(root, 'src', 'sprite-files.ts'), out);
