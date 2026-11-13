// The copy of the file manager tests: a run of claude in the project
// webshop, with seven changes as a real run leaves them: changed, new,
// deleted, one kept, one conflict, a binary file and a folder in a folder.
// The fake paddock answers from these files and changes them on apply,
// keep and discard.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Backup, BackupsDoc, Change, ChangesDoc, PointerDoc } from '../src/copy.js';
import { writeJson } from './helpers.js';

export const RUN = 'claude-webshop-7c1e42aa';
/** 13 November 2026, 14:05 UTC: the time of the pictures. */
export const NOW = Date.UTC(2026, 10, 13, 14, 5, 0);
const T = Math.floor(NOW / 1000);

const file = (size: number, ago: number): Change['copy'] => ({ size, mtime: T - ago, mode: '0644', link: null });

function change(path: string, kind: Change['kind'], o: Partial<Change> = {}): Change {
  return {
    path,
    kind,
    state: 'open',
    conflict: null,
    conflict_text: null,
    kept: false,
    writing: false,
    copy: kind === 'deleted' ? null : file(1840, 180),
    original: kind === 'new' ? null : file(1795, 86_400 * 3),
    text: true,
    added: 0,
    removed: 0,
    merge: null,
    ...o,
  };
}

export function demoChanges(project: string, copy: string): ChangesDoc {
  return {
    run: { id: RUN, short: '7c1e42', harness: 'claude', project, copy, state: 'live', started: T - 240, ended: null, in_place: false },
    pointer: { state: 'linked', text: 'The pointer and the store agree.', action: null, ok: true, store: '7c1e42' },
    token: 't1',
    counts: { new: 2, changed: 4, deleted: 1, conflicts: 1, kept: 1, ready: 5, applied: 0, behind: 0 },
    changes: [
      change('docs/README.md', 'changed', { state: 'conflict', conflict: 'both', conflict_text: 'both sides changed it', added: 6, removed: 2, merge: 'clean', original: file(2210, 900) }),
      change('logo.png', 'deleted', { text: false, added: null, removed: null, original: file(48_120, 86_400 * 40) }),
      change('src/cart.ts', 'changed', { added: 3, removed: 2 }),
      change('src/lib/util/format.ts', 'new', { added: 12, removed: 0, copy: file(310, 60) }),
      change('src/old.ts', 'changed', { state: 'kept', kept: true, added: 1, removed: 1 }),
      change('src/price.ts', 'changed', { added: 8, removed: 8 }),
      change('src/tax.ts', 'new', { added: 40, removed: 0, copy: file(1320, 120) }),
    ],
    applied: [],
    behind: [],
    special: [],
    auto_apply: false,
  };
}

export const CART_DIFF = `--- your folder/src/cart.ts
+++ the copy/src/cart.ts
@@ -38,7 +38,8 @@
 export function total(items: Item[]): number {
   if (items.length === 0) return 0;

   const total = items.reduce(
-    (s, i) => s + i.price, 0);
+    (s, i) => s + i.price * i.qty,
+    0);
   return total;
 }
@@ -61,7 +62,7 @@

 export function label(item: Item): string {
   const name = item.name.trim();
-  return name + ' x' + item.count + ' at the price of ' + format(item.price) + ' for each piece of it';
+  return name + ' x' + item.qty + ' at the price of ' + format(item.price) + ' for each piece of it';
 }

 export const EMPTY: Item[] = [];
`;

export const README_DIFF = `--- your folder/docs/README.md
+++ the copy/docs/README.md
@@ -1,5 +1,9 @@
 # webshop

-A small shop.
-Run it with npm start.
+A small shop with a cart and taxes.
+
+## Run it
+
+npm install
+npm start

`;

const backup = (id: string, kind: string, words: string, ago: number, files: number, bytes: number, shared: boolean, harness: string | null, o: Partial<Backup> = {}): Backup => ({
  id,
  short: id.slice(-6),
  kind,
  words,
  created: T - ago,
  run: harness ? `${harness}-webshop` : null,
  harness,
  files,
  bytes,
  shared,
  undone: null,
  undoable: kind !== 'start',
  ...o,
});

export function demoBackups(project: string): BackupsDoc {
  return {
    project,
    pointer: { state: 'linked', ok: true },
    backups: [
      backup('apply-20261113-140200', 'apply', 'before apply', 180, 3, 12_288, false, 'claude'),
      backup('start-20261113-134100', 'start', 'start of run', 1440, 412, 0, true, 'claude'),
      backup('restore-20261112-172000', 'restore', 'before restore', 74_700, 9, 49_152, false, null),
      backup('start-20261112-160500', 'start', 'start of run', 79_200, 409, 0, true, 'codex'),
    ],
    snapshots: [],
  };
}

export function demoPointer(project: string, home: string): PointerDoc {
  return {
    project,
    state: 'linked',
    text: 'The pointer and the store agree.',
    action: null,
    ok: true,
    pointer: { format: 1, store: '7c1e42', created: T - 86_400 * 9 },
    store: { id: '7c1e42', dir: `${home}/.local/share/paddock/copies/7c1e42`, original: project, runs: 3, copies: 1, backups: 4, size: { bytes: 61_440, kind: 'own' } },
    encryption: { state: 'encrypted', kind: 'LUKS' },
  };
}

/**
 * Put the copy into the folder of a fake world: one run of claude in
 * `DIR/code/webshop`, with its changes, diffs, backups and pointer. The
 * folder of the world is the home folder, so paths show with `~`.
 */
export function demoCopy(dir: string, edit?: (doc: ChangesDoc) => void): { project: string; doc: ChangesDoc } {
  const project = join(dir, 'code', 'webshop');
  mkdirSync(project, { recursive: true });
  process.env.HOME = dir;
  const doc = demoChanges(project, join(dir, '.local/share/paddock/copies/7c1e42/runs/7c1e42aa/tree'));
  edit?.(doc);
  writeJson(dir, 'ps.json', {
    api_version: 'paddock.ps/v1',
    sandboxes: [
      { id: RUN, short: '7c1e42', harness: 'claude', project, backend: 'bwrap', network: 'api', started: 240, activity: 'npm test', detached: true, view: true },
      { id: 'codex-docs-0b0b0b11', short: '0b0b0b', harness: 'codex', project: join(dir, 'code', 'docs'), backend: 'bwrap', network: 'api', started: 2000, activity: 'writing the guide', detached: true, view: true },
    ],
    ended: [],
  });
  writeJson(dir, 'board.json', { version: 1, projects: [] });
  writeJson(dir, 'approvals.json', { v: 1, pending: [] });
  writeJson(dir, 'mail.json', []);
  writeJson(dir, 'changes.json', doc);
  writeJson(dir, 'copies.json', { api_version: 'paddock.copies/v1', runs: [{ ...doc.run, counts: doc.counts }] });
  writeFileSync(join(dir, 'diff-src_cart.ts.txt'), CART_DIFF);
  writeFileSync(join(dir, 'diff-docs_README.md.txt'), README_DIFF);
  writeFileSync(join(dir, 'diff-src_tax.ts.txt'), `--- your folder/src/tax.ts\n+++ the copy/src/tax.ts\n@@ -0,0 +1,3 @@\n+export const VAT = 0.19;\n+\n+export const tax = (net: number): number => net * VAT;\n`);
  writeJson(dir, 'backups.json', { api_version: 'paddock.backups/v1', ...demoBackups(project) });
  writeJson(dir, 'pointer.json', { api_version: 'paddock.pointer/v1', ...demoPointer(project, dir) });
  return { project, doc };
}
