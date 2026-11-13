// A small world for the tests: two projects, a yard, a helper, a frozen
// run, an ended run, tools on your machine, in the room and on another
// computer, the build bench and the gate.
import { buildRuns } from '../src/model.js';
import { buildWorld, type World, type WorldExtra } from '../src/world/model.js';
import type { Board, MailBox, Pending, Ps } from '../src/types.js';

export const T = 1_800_000_000;

export function demo(): { ps: Ps; board: Board; pending: Pending[]; mail: MailBox[]; extra: WorldExtra } {
  const ps: Ps = {
    shared_devices: [],
    sandboxes: [
      {
        id: 'claude-alpha-71c3ac9d',
        short: '71c3ac',
        harness: 'claude',
        project: '/src/alpha',
        backend: 'bwrap',
        network: 'egress',
        started: T - 1000,
        activity: 'cargo test --lib board::',
        pasture: 'team',
        devices: [{ device: 'train', run: 'sandbox' }],
        timers: [{ name: 'build', left: 240 }],
      },
      { id: 'codex-alpha-0b0b0b11', short: '0b0b0b', harness: 'codex', project: '/src/alpha', backend: 'bwrap', network: 'api', approvals: 'manual', started: T - 2000, activity: 'waiting', pasture: 'team' },
      { id: 'gemini-alpha-0a1a2a33', short: '0a1a2a', harness: 'gemini', project: '/src/alpha', backend: 'bwrap', network: 'none', started: T - 300, parent: 'claude-alpha-71c3ac9d', spec_task: 'T012', activity: 'writing tests' },
      { id: 'grok-alpha-0f0f0f22', short: '0f0f0f', harness: 'grok', project: '/src/alpha', backend: 'podman', network: 'allowlist', started: T - 3000, frozen_since: T - 180, frozen_reason: 'ration is used up' },
      { id: 'opencode-web-0e0e0e33', short: '0e0e0e', harness: 'opencode', project: '/src/website', backend: 'bwrap', network: 'host', started: T - 4000, idle_since: T - 500 },
    ],
    ended: [{ id: 'codex-web-0d0d0d55', short: '0d0d0d', harness: 'codex', project: '/src/website', ended: T - 400, status: 'failed', text: 'tests failed' }],
  };
  const board: Board = {
    version: 1,
    approvals_waiting: 1,
    compile_slots: [
      { slot: 1, holder: 'claude-alpha-71c3ac' },
      { slot: 2, holder: 'grok-alpha-0f0f0f' },
    ],
    projects: [
      {
        name: 'alpha',
        path: '/src/alpha',
        loft: '/data/loft/abc/tree/.paddock',
        tickets: [
          { id: 'T-101', title: 'mail hooks', state: 'waiting', runs: [{ id: '0b0b0b', harness: 'codex', state: 'waiting', last: 'git push to origin/main' }] },
          { id: 'T-102', title: 'kv namespaces', state: 'working', runs: [{ id: '71c3ac', harness: 'claude', state: 'working', last: 'T-102: code and 14 tests pass' }] },
          { id: 'T-103', title: 'login refresh', state: 'open', runs: [] },
        ],
        runs_without_ticket: [],
      },
      { name: 'website', path: '/src/website', tickets: [], runs_without_ticket: [{ id: '0e0e0e', harness: 'opencode', state: 'idle', last: '' }] },
    ],
  };
  const pending: Pending[] = [
    { id: 'r-1', sandbox: 'codex-alpha-0b0b0b11', harness: 'codex', capability: 'exec', summary: 'git push', detail: 'git push origin main', created: T - 12, expires: T + 48, session_allowed: true },
  ];
  const mail: MailBox[] = [{ id: 'claude-alpha-71c3ac9d', short: '71c3ac', state: 'running', unread_up: 2 }];
  const extra: WorldExtra = {
    devices: {
      devices: [
        { name: 'screenshot', enabled: true, run: { run: 'host' }, scope: 'run', mode: 'ask' },
        { name: 'lint', enabled: true, run: { run: 'sandbox' }, scope: 'run', mode: 'allow' },
        { name: 'train', enabled: true, run: { run: 'remote', target: 'ssh://gpu-1', device: 'train' }, scope: 'run', mode: 'allow', grant: [{ grant: 'harness', to: 'claude' }] },
        { name: 'cache', enabled: true, run: { run: 'sandbox' }, scope: 'shared', mode: 'allow' },
        { name: 'off', enabled: false, run: { run: 'host' }, scope: 'run', mode: 'allow' },
      ],
    },
    kv: {
      pastures: [{ name: 'team', project: '/src/alpha', solo: false, members: [], keys: 4, leases: 1, ports: [{ port: 8080, sandbox: 'claude-alpha-71c3ac9d' }] }],
      user: { keys: 12, leases: 2 },
      namespaces: [{ name: 'ns:build', keys: 3, leases: 0 }],
    },
    files: { sandboxes: [{ id: 'claude-alpha-71c3ac9d', project_changes: { git: true, changed: ['src/a.rs', 'src/b.rs', 'README.md'] } }] },
    forage: {
      windows: [
        { agent: 'claude', label: '5h', used: 35 },
        { agent: 'codex', label: 'week', used: 89, overgrazing: true },
      ],
    },
  };
  return { ps, board, pending, mail, extra };
}

/** The demo world, after an edit of its JSON. */
export function demoWorld(edit?: (d: ReturnType<typeof demo>) => void): World {
  const d = demo();
  edit?.(d);
  return buildWorld(buildRuns(d.ps, d.board, d.pending, d.mail), d.ps, d.board, d.pending, d.extra);
}

export const CLAUDE = 'claude-alpha-71c3ac9d';
export const CODEX = 'codex-alpha-0b0b0b11';
export const GEMINI = 'gemini-alpha-0a1a2a33';
export const GROK = 'grok-alpha-0f0f0f22';
export const OPENCODE = 'opencode-web-0e0e0e33';
