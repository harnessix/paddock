#!/usr/bin/env node
// Single binaries of the terminal UI with `bun build --compile`, for the
// release assets: paddock-tui-linux-x64, -linux-arm64, -darwin-arm64.
// Usage: node scripts/compile.mjs [TARGET...]   (default: all three)
import { execFileSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const ALL = ['linux-x64', 'linux-arm64', 'darwin-arm64'];
const targets = process.argv.slice(2).length > 0 ? process.argv.slice(2) : ALL;
for (const t of targets) {
  if (!ALL.includes(t)) {
    console.error(`unknown target ${t}: one of ${ALL.join(', ')}`);
    process.exit(2);
  }
}
execFileSync('npm', ['run', 'build'], { cwd: root, stdio: 'inherit' });
mkdirSync(join(root, 'bin'), { recursive: true });
for (const t of targets) {
  const out = join(root, 'bin', `paddock-tui-${t}`);
  execFileSync('bun', [join(root, 'scripts', 'bun-build.mjs'), t, out], { cwd: root, stdio: 'inherit' });
  console.log(out);
}
