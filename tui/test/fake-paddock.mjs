#!/usr/bin/env node
// A fake paddock for the tests: it answers from the JSON files in
// $FAKE_PADDOCK_DIR and records every call (arguments and stdin) in
// calls.jsonl there.
import { appendFileSync, existsSync, readFileSync, readSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { isatty } from 'node:tty';

const dir = process.env.FAKE_PADDOCK_DIR;
const args = process.argv.slice(2);
let stdin = '';
// isatty, not process.stdin: Node makes the input non-blocking when it opens it, and the fake run reads a line with a plain read.
if (!isatty(0)) {
  try {
    stdin = readFileSync(0, 'utf8');
  } catch {
    stdin = '';
  }
}
// `paddock undo` means the folder that it runs in: its record has that folder.
appendFileSync(join(dir, 'calls.jsonl'), JSON.stringify(args[0] === 'undo' ? { args, stdin, cwd: process.cwd() } : { args, stdin }) + '\n');

function file(name, fallback) {
  const p = join(dir, name);
  process.stdout.write(existsSync(p) ? readFileSync(p, 'utf8') : JSON.stringify(fallback));
  process.exit(0);
}

// A file `unsupported-CMD` makes CMD fail as in an older paddock (clap
// usage error, exit 2). A file `fail-CMD` makes it fail with an error.
if (existsSync(join(dir, `unsupported-${args[0]}`))) {
  process.stderr.write(`error: unrecognized subcommand '${args[0]}'\n\nUsage: paddock <COMMAND>\n\nFor more information, try '--help'.\n`);
  process.exit(2);
}
if (existsSync(join(dir, `fail-${args[0]}`))) {
  process.stderr.write('error: the state is locked\n');
  process.exit(1);
}

const a = args.join(' ');
if (a === 'ps --json') file('ps.json', { api_version: 'paddock.ps/v1', sandboxes: [], ended: [] });
if (a === 'board --json') file('board.json', { version: 1, projects: [] });
if (a === 'approvals --json') file('approvals.json', { v: 1, pending: [] });
if (a === 'mail ls --json') file('mail.json', []);
if (a === 'list --json') file('list.json', { harnesses: [] });
if (a === 'device ls --json') file('device.json', { mount: false, builtin: [], devices: [] });
if (a === 'kv overview --json') file('kv.json', { api_version: 'paddock.kv/v1', pastures: [], user: { keys: 0, leases: 0 }, namespaces: [], legacy: [] });
if (a === 'files --json') file('files.json', { api_version: 'paddock.files/v1', sandboxes: [], scratch: [] });
if (a === 'forage --json') file('forage.json', { api_version: 'paddock.forage/v1', windows: [] });
if (a === 'horse list --json') file('horse.json', []);
if (args[0] === 'mail' && args[1] === 'ls' && args[3] === '--json') file('messages.json', []);
if (args[0] === 'mail' && args[1] === 'show') file('message.json', { seq: Number(args[3]), text: '' });
if (args[0] === 'view' && args.includes('--tail')) file('tail.json', { lines: [] });
if (a === 'split --help') process.exit(process.env.FAKE_PADDOCK_SPLIT === '1' ? 0 : 2);
if (args[0] === 'stop' && args[1] === 'fail-me') {
  process.stderr.write('error: \u001b[31mno sandbox\u001b[0m with that id\n');
  process.exit(1);
}
if (a === 'recent --json') file('recent.json', { api_version: 'paddock.recent/v1', projects: [] });
// `explain HARNESS FOLDER --json`: explain.json, or explain-NAME.json for
// the folder with that last name. A refused folder fails after the JSON,
// as in paddock. With a file `explain-exit`, the command fails without JSON.
if (args[0] === 'explain' && args.includes('--json')) {
  if (existsSync(join(dir, 'explain-exit'))) {
    process.stderr.write('error: no backend can run this sandbox\n');
    process.exit(2);
  }
  const name = `explain-${String(args[2] ?? '').split('/').pop()}.json`;
  const p = existsSync(join(dir, name)) ? join(dir, name) : join(dir, 'explain.json');
  const text = existsSync(p) ? readFileSync(p, 'utf8') : JSON.stringify({ network: 'egress', approvals: 'manual', folder_check: { level: 'ok', reason: '', fix: '' } });
  process.stdout.write(text);
  let level = '';
  try {
    level = JSON.parse(text).folder_check?.level ?? '';
  } catch {
    level = '';
  }
  if (level === 'refuse' || level === 'confirm') {
    process.stderr.write('error: the folder guard refused the folder\n');
    process.exit(2);
  }
  process.exit(0);
}
// A run in the terminal (not --detach), or a command that the home screen
// hands the terminal to: it writes a line, reads one line from the terminal
// when a file `run-reads` is there, and ends with the code in `run-exit`.
if ((args[0] === 'run' && !args.includes('--detach')) || args[0] === 'doctor' || args[0] === 'setup' || (args[0] === 'app' && args[1] === 'run')) {
  process.stdout.write(`FAKE ${args.join(' ')} START=${process.env.PADDOCK_TUI_START ?? 'unset'}\n`);
  if (existsSync(join(dir, 'run-reads'))) {
    process.stdout.write('fake> ');
    const buf = Buffer.alloc(256);
    let n = 0;
    try {
      n = readSync(0, buf, 0, 256, null);
    } catch {
      n = 0;
    }
    appendFileSync(join(dir, 'run-input.txt'), buf.subarray(0, n).toString('utf8'));
  }
  const codeFile = join(dir, 'run-exit');
  process.exit(existsSync(codeFile) ? Number(readFileSync(codeFile, 'utf8')) : 0);
}

// ---- the copy of a run (COPY1) ----
// `changes.json` is the state of the one copy of the test. apply, keep
// and discard change it, as paddock changes the copy, and print the JSON
// of their result. A file `stale-apply` makes the next apply stop, as
// when the list is not the one of the token; `changes-next.json` is the
// list from then on.
function readJson(name, fallback) {
  const p = join(dir, name);
  return existsSync(p) ? JSON.parse(readFileSync(p, 'utf8')) : fallback;
}
function out(v, code = 0) {
  process.stdout.write(JSON.stringify(v));
  process.exit(code);
}
function fail(text, code = 1) {
  process.stderr.write(`error: ${text}\n`);
  process.exit(code);
}
/** The arguments after `--`, and the value of an option before it. */
const dashes = args.indexOf('--');
const paths = dashes >= 0 ? args.slice(dashes + 1) : [];
const option = (name) => {
  const i = args.indexOf(name);
  return i >= 0 && (dashes < 0 || i < dashes) ? args[i + 1] : undefined;
};
const covers = (sel, path) => sel === path || path.startsWith(sel.endsWith('/') ? sel : `${sel}/`);
function saveChanges(doc) {
  const c = doc.changes;
  const open = (k) => c.filter((x) => x.kind === k).length;
  doc.counts = {
    new: open('new'),
    changed: open('changed'),
    deleted: open('deleted'),
    conflicts: c.filter((x) => x.conflict !== null).length,
    kept: c.filter((x) => x.kept).length,
    ready: c.filter((x) => !x.kept && x.conflict === null).length,
    applied: doc.applied.length,
    behind: (doc.behind ?? []).length,
  };
  doc.token = `t${Number(String(doc.token).replace(/\D/g, '') || 0) + 1}`;
  writeFileSync(join(dir, 'changes.json'), JSON.stringify(doc));
  // The list of the runs has the same counts.
  const copies = readJson('copies.json', null);
  if (copies) {
    for (const r of copies.runs) if (r.id === doc.run.id) r.counts = doc.counts;
    writeFileSync(join(dir, 'copies.json'), JSON.stringify(copies));
  }
}
if (a === 'changes --json') file('copies.json', { api_version: 'paddock.copies/v1', runs: [] });
if (args[0] === 'changes' && args[2] === '--json') {
  if (!existsSync(join(dir, 'changes.json'))) fail(`no run on a copy has the id ${args[1]}. \`paddock changes\` lists them`);
  file('changes.json', {});
}
if (args[0] === 'changes' && String(args[2]).startsWith('--diff=')) {
  const name = `diff-${args[2].slice(7).replace(/[^A-Za-z0-9.]+/g, '_')}.txt`;
  process.stdout.write(existsSync(join(dir, name)) ? readFileSync(join(dir, name), 'utf8') : '');
  process.exit(0);
}
if (args[0] === 'apply' && args.includes('--json')) {
  if (!args.includes('--yes')) fail('--json needs --yes', 2);
  if (existsSync(join(dir, 'stale-apply'))) {
    unlinkSync(join(dir, 'stale-apply'));
    if (existsSync(join(dir, 'changes-next.json'))) writeFileSync(join(dir, 'changes.json'), readFileSync(join(dir, 'changes-next.json')));
    fail('the changes are not the ones that you saw: a file changed since then. List them again, then apply');
  }
  const doc = readJson('changes.json', null);
  if (!doc) fail(`no run on a copy has the id ${args[1]}`);
  const token = option('--token');
  if (token !== undefined && token !== doc.token) fail('the changes are not the ones that you saw: a file changed since then. List them again, then apply');
  const on = option('--on-conflict') ?? 'stop';
  const done = { written: [], deleted: [], merged: [], both: [], kept_original: [], conflicts: [], kept: [], unsafe_links: [], backup: null };
  const left = [];
  for (const c of doc.changes) {
    const picked = paths.length > 0 ? paths.some((p) => covers(p, c.path)) : true;
    if (!picked) left.push(c);
    else if (paths.length === 0 && c.kept) {
      done.kept.push(c.path);
      left.push(c);
    } else if (c.conflict !== null && on === 'stop') {
      done.conflicts.push(c.path);
      left.push(c);
    } else if (c.conflict !== null && on === 'original') {
      done.kept_original.push(c.path);
      left.push(c);
    } else if (c.conflict !== null && on === 'both') done.both.push([c.path, `${c.path}.conflict-7c1e42-1402`]);
    else if (c.conflict !== null && on === 'merge') done.merged.push(c.path);
    else if (c.kind === 'deleted') done.deleted.push(c.path);
    else done.written.push(c.path);
  }
  const applied = [...done.written, ...done.deleted, ...done.merged];
  if (applied.length > 0) done.backup = 'apply-20261113-140200';
  doc.changes = left;
  doc.applied = [...doc.applied, ...applied.filter((p) => !done.deleted.includes(p))];
  saveChanges(doc);
  out({ ...done, api_version: 'paddock.apply/v1', run: doc.run.id, project: doc.run.project }, done.conflicts.length > 0 ? 1 : 0);
}
if (args[0] === 'keep' && args.includes('--json')) {
  const doc = readJson('changes.json', null);
  if (!doc) fail(`no run on a copy has the id ${args[1]}`);
  const off = args.includes('--off') && (dashes < 0 || args.indexOf('--off') < dashes);
  const done = [];
  for (const c of doc.changes) {
    if (!paths.some((p) => covers(p, c.path))) continue;
    c.kept = !off;
    c.state = c.conflict !== null ? 'conflict' : off ? 'open' : 'kept';
    done.push(c.path);
  }
  saveChanges(doc);
  out({ kept: !off, paths: done, api_version: 'paddock.keep/v1', run: doc.run.id, project: doc.run.project });
}
if (args[0] === 'discard' && args.includes('--json')) {
  const doc = readJson('changes.json', null);
  if (!doc) fail(`no run on a copy has the id ${args[1]}`);
  // Without a path, paddock removes the whole copy. The interface never asks for that.
  if (paths.length === 0) {
    writeFileSync(join(dir, 'copy-removed'), '');
    out({ copy_removed: true, paths: [], api_version: 'paddock.discard/v1', run: doc.run.id, project: doc.run.project });
  }
  const done = doc.changes.filter((c) => paths.some((p) => covers(p, c.path))).map((c) => c.path);
  doc.changes = doc.changes.filter((c) => !done.includes(c.path));
  saveChanges(doc);
  out({ copy_removed: false, paths: done, api_version: 'paddock.discard/v1', run: doc.run.id, project: doc.run.project });
}
if (args[0] === 'undo' && args.includes('--json')) {
  if (!args.includes('--yes')) fail('--json needs --yes', 2);
  out(readJson('undo.json', { of: 'apply-20261113-140200', restored: ['src/cart.ts', 'src/price.ts'], removed: ['src/tax.ts'], skipped: [], backup: null, api_version: 'paddock.undo/v1', project: process.cwd() }));
}
if (args[0] === 'backups' && args.includes('--json')) file('backups.json', { api_version: 'paddock.backups/v1', project: args[1], pointer: { state: 'linked', ok: true }, backups: [], snapshots: [] });
if (args[0] === 'restore' && args.includes('--json')) {
  if (!args.includes('--yes')) fail('--json needs --yes', 2);
  out(readJson('restore.json', { of: args[1], restored: ['src/cart.ts', 'src/price.ts', 'README.md'], removed: [], skipped: [], backup: 'restore-20261113-140300', api_version: 'paddock.restore/v1', project: option('--project'), newer: [] }));
}
if (args[0] === 'pointer' && args[1] === 'status' && args.includes('--json')) file('pointer.json', { api_version: 'paddock.pointer/v1', project: args[2], state: 'no pointer', text: 'The folder has no .paddock/pointer.json.', action: 'paddock pointer create', ok: false, pointer: null, store: null, encryption: { state: 'unknown' } });
if (args[0] === 'pointer' && ['new', 'create', 'link'].includes(args[1])) {
  // The folder has its store from now on.
  if (existsSync(join(dir, 'pointer-after.json'))) writeFileSync(join(dir, 'pointer.json'), readFileSync(join(dir, 'pointer-after.json')));
  process.stdout.write(args[1] === 'create' ? `Made ${args[2]}/.paddock/pointer.json and the store /home/demo/.local/share/paddock/copies/7c1e42.\n` : 'Made the store /home/demo/.local/share/paddock/copies/9d0a11.\n');
  process.exit(0);
}
process.stdout.write(`ok ${args[0]}\n`);
process.exit(0);
