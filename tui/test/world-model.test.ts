// The world model: every fact comes from a field of paddock's JSON, and a
// fact that paddock does not report stays out.
import { describe, expect, test } from 'vitest';
import { buildRuns } from '../src/model.js';
import { buildWorld, findRoom, granted, netMode, peerName } from '../src/world/model.js';
import { CLAUDE, CODEX, demo, demoWorld, GEMINI, GROK, OPENCODE } from './world-demo.js';

describe('the world from paddock\'s JSON', () => {
  test('a house per project in the order of the board, a room per run', () => {
    const w = demoWorld();
    expect(w.houses.map((h) => [h.name, h.path])).toEqual([
      ['alpha', '/src/alpha'],
      ['website', '/src/website'],
    ]);
    // Live rooms by their start, a helper right after the run that started it, ended rooms last.
    expect(w.houses[0]?.rooms.map((r) => r.name)).toEqual(['grok-0f0f0f', 'codex-0b0b0b', 'claude-71c3ac', 'gemini-0a1a2a']);
    expect(w.houses[1]?.rooms.map((r) => [r.name, r.run.live])).toEqual([
      ['opencode-0e0e0e', true],
      ['codex-0d0d0d', false],
    ]);
    expect(w.waiting).toBe(1);
  });

  test('one loft per project, from the board; none when the board has none', () => {
    const w = demoWorld();
    expect(w.houses[0]?.loft).toBe('/data/loft/abc/tree/.paddock');
    expect(w.houses[1]?.loft).toBe('');
  });

  test('a room: its request, its network mode, its mail, its helper, its yard, its changed files', () => {
    const w = demoWorld();
    const codex = findRoom(w, CODEX)?.room;
    expect(codex?.asks.map((p) => p.id)).toEqual(['r-1']);
    expect(codex?.net).toBe('api');
    expect(codex?.yard).toBe('team');
    expect(codex?.changed).toBeNull();
    const claude = findRoom(w, CLAUDE)?.room;
    expect(claude).toMatchObject({ net: 'egress', unread: 2, children: [GEMINI], changed: 3, yard: 'team' });
    expect(claude?.timers).toEqual([{ name: 'build', left: 240 }]);
    expect(findRoom(w, GEMINI)?.room).toMatchObject({ parent: CLAUDE, specTask: 'T012', net: 'none', yard: '' });
    expect(findRoom(w, GROK)?.room).toMatchObject({ net: 'allowlist', frozenReason: 'ration is used up' });
    expect(findRoom(w, GROK)?.room.run.frozen).toBe(true);
    expect(findRoom(w, OPENCODE)?.room.net).toBe('host');
  });

  test('the five network modes, the app mode, and a word that paddock does not have', () => {
    expect(['none', 'api', 'allowlist', 'egress', 'host', 'ask', 'vpn', ''].map(netMode)).toEqual(['none', 'api', 'allowlist', 'egress', 'host', 'ask', 'unknown', 'unknown']);
  });

  test('tools: where each runs, who can reach it, and what runs now', () => {
    const w = demoWorld();
    const claude = findRoom(w, CLAUDE)?.room;
    // Your machine first, then the other computer, then the tools in the room. A tool that is off does not show.
    expect(claude?.tools.map((t) => [t.name, t.run, t.peer, t.ask, t.on, t.shared])).toEqual([
      ['screenshot', 'host', '', true, false, false],
      ['train', 'remote', 'gpu-1', false, true, false],
      ['cache', 'sandbox', '', false, false, true],
      ['lint', 'sandbox', '', false, false, false],
    ]);
    // train is granted to claude only.
    expect(findRoom(w, CODEX)?.room.tools.map((t) => t.name)).toEqual(['screenshot', 'cache', 'lint']);
    // An ended run reaches nothing.
    expect(w.houses[1]?.rooms[1]?.tools).toEqual([]);
    expect(w.houses[0]?.hall.map((t) => t.name)).toEqual(['cache']);
  });

  test('a grant decides who reaches a tool', () => {
    const r = { harness: 'claude', project: '/src/alpha', yard: 'team' };
    expect(granted({ name: 'a' }, r)).toBe(true);
    expect(granted({ name: 'a', grant: [] }, r)).toBe(true);
    expect(granted({ name: 'a', grant: [{ grant: 'all' }] }, r)).toBe(true);
    expect(granted({ name: 'a', grant: [{ grant: 'harness', to: 'codex' }] }, r)).toBe(false);
    expect(granted({ name: 'a', grant: [{ grant: 'harness', to: 'codex' }, { grant: 'pasture', to: 'team' }] }, r)).toBe(true);
    expect(granted({ name: 'a', grant: [{ grant: 'project', to: '/src/alpha' }] }, r)).toBe(true);
    expect(granted({ name: 'a', grant: [{ grant: 'project', to: 'alpha' }] }, r)).toBe(true);
    expect(granted({ name: 'a', grant: [{ grant: 'pasture', to: '' }] }, { ...r, yard: '' })).toBe(false);
    expect(granted({ name: 'a', grant: [{ grant: 'remote', to: 'x' }] }, r)).toBe(false);
  });

  test('yards, the build bench, the islands and the gate', () => {
    const w = demoWorld();
    expect(w.houses[0]?.yards).toEqual([{ name: 'team', project: '/src/alpha', members: [CODEX, CLAUDE], keys: 4, leases: 1, ports: [{ port: 8080, sandbox: CLAUDE }] }]);
    expect(w.bench.seats).toEqual([
      { slot: 1, holder: 'claude-alpha-71c3ac' },
      { slot: 2, holder: 'grok-alpha-0f0f0f' },
    ]);
    expect(w.islands).toEqual([{ name: 'gpu-1', target: 'ssh://gpu-1', tools: ['train'] }]);
    expect(w.gate).toMatchObject({ keys: 12, leases: 2, cabinets: [{ name: 'ns:build', keys: 3, leases: 0 }] });
    expect(w.gate.forage).toHaveLength(2);
    expect(w.stable).toBeNull();
    expect(['ssh://gpu-1', 'ssh://me@box.lan:22/x', 'unix:/run/peer.sock', ''].map(peerName)).toEqual(['gpu-1', 'box.lan', 'peer.sock', 'other computer']);
  });

  test('the stable shows only with a saved session: a stall per horse, the newest session first', () => {
    const w = demoWorld((d) => {
      d.extra.horses = [
        { id: 's1', horse: 'bay', created_ms: 10 },
        { id: 's2', horse: 'bay', created_ms: 30 },
        { id: 's3', horse: 'roan', created_ms: 20 },
      ];
    });
    expect(w.stable?.stalls.map((s) => [s.horse, s.sessions.map((x) => x.id)])).toEqual([
      ['bay', ['s2', 's1']],
      ['roan', ['s3']],
    ]);
  });

  test('a gap stays a gap: without the extra reads nothing is made up', () => {
    const d = demo();
    const w = buildWorld(buildRuns(d.ps, d.board, d.pending, d.mail), d.ps, d.board, d.pending);
    expect(w.gate).toEqual({ keys: null, leases: null, cabinets: [], forage: null });
    expect(w.islands).toEqual([]);
    expect(findRoom(w, CLAUDE)?.room).toMatchObject({ tools: [], changed: null });
    // The yard is known from ps; its notes are not.
    expect(w.houses[0]?.yards[0]).toMatchObject({ name: 'team', keys: null, leases: null, ports: [] });
    // No board: the houses come from the runs, and no loft is assumed.
    const bare = buildWorld(buildRuns(d.ps, undefined, [], []), d.ps, undefined, []);
    expect(bare.houses.map((h) => [h.name, h.loft, h.tickets.length])).toEqual([
      ['alpha', '', 0],
      ['website', '', 0],
    ]);
    expect(bare.bench.seats).toEqual([]);
  });

  test('names from a sandbox are cleaned', () => {
    const w = demoWorld((d) => {
      const s = d.ps.sandboxes[0];
      if (s) s.pasture = 'te\u001b[31mam';
      d.extra.devices?.devices.push({ name: 'x\u001b]0;pwned\u0007y', enabled: true, run: { run: 'remote', target: 'ssh://h\u001b[2Jost' } });
    });
    expect(findRoom(w, CLAUDE)?.room.yard).toBe('team');
    expect(findRoom(w, CLAUDE)?.room.tools.map((t) => t.name)).toContain('xy');
    expect(w.islands.map((i) => i.name)).toContain('host');
  });
});
