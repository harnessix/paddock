// A ranch as a real one looks: six projects, three runs that work, one
// that waits for you, one that is paused, a helper on its path, a shared
// yard with a port, a tool on another computer, a project with tickets
// and no run, and a project with a long name and nothing at all.
import { buildRuns } from '../src/model.js';
import { buildWorld, type World, type WorldExtra } from '../src/world/model.js';
import type { Board, MailBox, Pending, Ps } from '../src/types.js';
import { T } from './world-demo.js';

export const CORE = '/home/demo/src/paddock-core';
export const R_CLAUDE = 'claude-core-a1b2c3d4';
export const R_GEMINI = 'gemini-core-0a1a2a33';
export const R_CODEX = 'codex-web-0b0b0b11';
export const R_OPENCODE = 'opencode-web-0e0e0e33';
export const R_GROK = 'grok-trainer-0f0f0f22';
export const R_COPILOT = 'copilot-notes-0c0c0c44';

export function ranch(): { ps: Ps; board: Board; pending: Pending[]; mail: MailBox[]; extra: WorldExtra } {
  const ps: Ps = {
    shared_devices: [],
    sandboxes: [
      { id: R_CLAUDE, short: 'a1b2c3', harness: 'claude', project: CORE, backend: 'bwrap', network: 'api', started: T - 5400, activity: 'cargo test --workspace: 412 of 600 tests pass, the suite of the board runs now' },
      { id: R_GEMINI, short: '0a1a2a', harness: 'gemini', project: CORE, backend: 'bwrap', network: 'none', started: T - 300, parent: R_CLAUDE, spec_task: 'T031', activity: 'writing tests' },
      { id: R_CODEX, short: '0b0b0b', harness: 'codex', project: '/home/demo/src/website', backend: 'bwrap', network: 'egress', started: T - 2000, activity: 'npm run build', pasture: 'web' },
      { id: R_OPENCODE, short: '0e0e0e', harness: 'opencode', project: '/home/demo/src/website', backend: 'bwrap', network: 'allowlist', approvals: 'manual', started: T - 1500, activity: 'waiting', pasture: 'web' },
      { id: R_GROK, short: '0f0f0f', harness: 'grok', project: '/home/demo/src/trainer', backend: 'podman', network: 'api', started: T - 9000, frozen_since: T - 180, frozen_reason: 'ration is used up', devices: [{ device: 'train', run: 'remote' }] },
      { id: R_COPILOT, short: '0c0c0c', harness: 'copilot', project: '/home/demo/src/notes-app', backend: 'bwrap', network: 'api', started: T - 700, activity: 'reading the schema' },
    ],
    ended: [],
  };
  const ticket = (id: string, state: string): { id: string; title: string; state: string; runs: never[] } => ({ id, title: `the work of ${id}`, state, runs: [] });
  const board: Board = {
    version: 1,
    approvals_waiting: 1,
    compile_slots: [{ slot: 1, holder: 'claude-core-a1b2c3' }],
    projects: [
      {
        name: 'paddock-core',
        path: CORE,
        loft: '/data/loft/core/tree/.paddock',
        tickets: [
          { id: 'WORLD1', title: 'the world', state: 'working', runs: [{ id: 'a1b2c3', harness: 'claude', state: 'working', last: 'WORLD1: the ranch draws, 412 of 600 tests pass, the suite of the board runs now and the report follows' }] },
          ...['ACL1', 'COORD1', 'COPY1', 'ENV1', 'FLOW2', 'HORSE3', 'JOIN1', 'KV7', 'LOFT2', 'MAIL4'].map((id) => ticket(id, 'open')),
        ],
        runs_without_ticket: [{ id: '0a1a2a', harness: 'gemini', state: 'working', last: 'writing tests' }],
      },
      {
        name: 'website',
        path: '/home/demo/src/website',
        tickets: [{ id: 'WEB-12', title: 'the pricing page', state: 'waiting', runs: [{ id: '0e0e0e', harness: 'opencode', state: 'waiting', last: 'git push to origin/main' }] }],
        runs_without_ticket: [{ id: '0b0b0b', harness: 'codex', state: 'working', last: 'npm run build' }],
      },
      { name: 'trainer', path: '/home/demo/src/trainer', loft: '/data/loft/trainer/tree/.paddock', tickets: [], runs_without_ticket: [{ id: '0f0f0f', harness: 'grok', state: 'idle', last: '' }] },
      { name: 'notes-app', path: '/home/demo/src/notes-app', tickets: [], runs_without_ticket: [{ id: '0c0c0c', harness: 'copilot', state: 'working', last: 'reading the schema' }] },
      { name: 'docs', path: '/home/demo/src/docs', tickets: [ticket('DOC-3', 'open'), ticket('DOC-4', 'open')], runs_without_ticket: [] },
      { name: 'infrastructure-scripts-and-dotfiles', path: '/home/demo/src/infrastructure-scripts-and-dotfiles', tickets: [], runs_without_ticket: [] },
    ],
  };
  const pending: Pending[] = [{ id: 'r-1', sandbox: R_OPENCODE, harness: 'opencode', capability: 'exec', summary: 'git push', detail: 'git push origin main', created: T - 12, expires: T + 48, session_allowed: true }];
  const mail: MailBox[] = [{ id: R_CLAUDE, short: 'a1b2c3', state: 'running', unread_up: 2 }];
  const extra: WorldExtra = {
    devices: {
      devices: [
        { name: 'screenshot', enabled: true, run: { run: 'host' }, scope: 'run', mode: 'ask', grant: [{ grant: 'harness', to: 'claude' }] },
        { name: 'train', enabled: true, run: { run: 'remote', target: 'ssh://gpu-1', device: 'train' }, scope: 'run', mode: 'allow', grant: [{ grant: 'harness', to: 'grok' }] },
      ],
    },
    kv: {
      pastures: [{ name: 'web', project: '/home/demo/src/website', solo: false, members: [], keys: 4, leases: 1, ports: [{ port: 3000, sandbox: R_CODEX }] }],
      user: { keys: 12, leases: 2 },
      namespaces: [{ name: 'ns:build', keys: 3, leases: 0 }],
    },
    forage: {
      windows: [
        { agent: 'claude', label: '5h', used: 35 },
        { agent: 'codex', label: 'week', used: 91, overgrazing: true },
      ],
    },
  };
  return { ps, board, pending, mail, extra };
}

/** The ranch, after an edit of its JSON. */
export function ranchWorld(edit?: (d: ReturnType<typeof ranch>) => void): World {
  const d = ranch();
  edit?.(d);
  return buildWorld(buildRuns(d.ps, d.board, d.pending, d.mail), d.ps, d.board, d.pending, d.extra);
}
