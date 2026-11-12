import { readFileSync } from 'node:fs';
import { expect, test } from 'vitest';
import { VERSION } from '../src/version.js';

test('the package has the version of paddock', () => {
  const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as { version: string };
  const cargo = readFileSync(new URL('../../Cargo.toml', import.meta.url), 'utf8');
  const m = /^version = "([^"]+)"/m.exec(cargo);
  expect(pkg.version).toBe(VERSION);
  expect(m?.[1]).toBe(VERSION);
});
