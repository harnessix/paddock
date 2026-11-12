#!/usr/bin/env node
// A fake paddock for the tests: it answers from the JSON files in
// $FAKE_PADDOCK_DIR and records every call (arguments and stdin) in
// calls.jsonl there.
import { appendFileSync, existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const dir = process.env.FAKE_PADDOCK_DIR;
const args = process.argv.slice(2);
let stdin = '';
if (!process.stdin.isTTY) {
  try {
    stdin = readFileSync(0, 'utf8');
  } catch {
    stdin = '';
  }
}
appendFileSync(join(dir, 'calls.jsonl'), JSON.stringify({ args, stdin }) + '\n');

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
if (args[0] === 'mail' && args[1] === 'ls' && args[3] === '--json') file('messages.json', []);
if (args[0] === 'mail' && args[1] === 'show') file('message.json', { seq: Number(args[3]), text: '' });
if (args[0] === 'view' && args.includes('--tail')) file('tail.json', { lines: [] });
if (a === 'split --help') process.exit(process.env.FAKE_PADDOCK_SPLIT === '1' ? 0 : 2);
if (args[0] === 'stop' && args[1] === 'fail-me') {
  process.stderr.write('error: \u001b[31mno sandbox\u001b[0m with that id\n');
  process.exit(1);
}
process.stdout.write(`ok ${args[0]}\n`);
process.exit(0);
