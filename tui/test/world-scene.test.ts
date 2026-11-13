// The pictures of the world: the ranch, a building, a room in the simple
// and the Plumbing view, the loft, the Inspector. The text form (no art)
// shows what each picture says; the art form shows that amber keeps its
// one meaning and that the picture fits the screen.
import { describe, expect, test } from 'vitest';
import { cells } from '../src/clean.js';
import { rowText } from '../src/world/canvas.js';
import { inspect, inspectorLines, wrap } from '../src/world/inspect.js';
import { Motion } from '../src/world/motion.js';
import { AMBER, AMBER_DARK, AMBER_SHADE } from '../src/world/pixels.js';
import { breadcrumb, drawWorld, infoLine, LINK, moveSelection, panelLines, type Hit, type Level, type View } from '../src/world/scene.js';
import type { World } from '../src/world/model.js';
import { CLAUDE, CODEX, demoWorld, GEMINI, GROK, OPENCODE, T } from './world-demo.js';
import type { ArtMode } from '../src/art.js';

const ALPHA = '/src/alpha';

function view(level: Level, o: Partial<View> = {}): View {
  return { level, house: level === 'ranch' ? '' : ALPHA, room: '', sel: '', plumbing: false, ...o };
}

function draw(world: World, v: View, o: { w?: number; h?: number; mode?: ArtMode; tick?: number; motion?: Motion; plain?: boolean } = {}) {
  return drawWorld(world, { w: o.w ?? 80, h: o.h ?? 21, mode: o.mode ?? 'none', plain: o.plain, tick: o.tick ?? 0, motion: o.motion ?? new Motion(), view: v, t: T });
}

function text(world: World, v: View, o: Parameters<typeof draw>[2] = {}): string {
  return draw(world, v, o).canvas.lines().join('\n');
}

/** Whether a colour is in any cell of the picture. */
function has(world: World, v: View, rgb: readonly number[], o: Parameters<typeof draw>[2] = {}): boolean {
  const code = `2;${rgb[0]};${rgb[1]};${rgb[2]}`;
  return draw(world, v, { mode: 'truecolor', ...o })
    .canvas.rows()
    .some((r) => r.st.some((s) => s.includes(code)));
}

/**
 * Amber: the one of the lines, the words and the layers, its shade in the layers, the dark one of a light terminal.
 * Not its light shade: the character of a run that waits has its mark in it, in any world.
 */
const AMBERS = [AMBER, AMBER_SHADE, AMBER_DARK];
const sgr = (rgb: readonly number[]): string => `2;${rgb[0]};${rgb[1]};${rgb[2]}`;
const amber = (world: World, v: View, o: Parameters<typeof draw>[2] = {}): boolean => AMBERS.some((a) => has(world, v, a, o));

describe('the ranch', () => {
  test('80 columns: projects as houses, runs as rooms, the yard, the helper on its path, the frozen and the ended room, the bench', () => {
    const f = text(demoWorld(), view('ranch', { sel: `room:${CODEX}` }));
    expect(f).toMatchInlineSnapshot(`
      "Ranch                                                               ◆ 1 asks you
       ▄▄ alpha ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄ loft ▲ ▄
       █ ▛▀▀▀▀▀▀▀▀▀▀▜  ▛▀▀▀▀▀▀▀▀▀▀▜  ▛▀▀▀▀▀▀▀▀▀▀▜  ▛▀▀▀▀▀▀▀▀▀▀▜ ◆ waiting T-101   █
       █ ▌[list]    ▐  ▌[model]   ▐  ▌[open]    ▐  ▌[closed]  ▐ ● working T-102   █
       █ ▌ ❄        ▐  ▌ ◕!       ▐  ▌ ◕   ⚑2   ▐  ▌ ◕        ▐ ◇ open T-103      █
       █ ▌▟█▙  ⏚!   ▐  ▐▟█▙  ⏚!   ▐  ▌▟█▙  ⏚!   ▐··▌▟█▙  ⏚!   ▐                   █
       █ ▌paused    ▐  ▌asks you  ▐  ▌running   ▐  ▌running   ▐                   █
       █ ▙▄▄▄▄▄▄▄▄▄▄▟  ▙▄▄▄▄▄▄▄▄▄▄▟  ▙▄▄▄▄▄▄▄▄▄▄▟  ▙▄▄▄▄▄▄▄▄▄▄▟                   █
       █ grok          codex ◆       claude ✉2     gemini                         █
       ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀┄┄yard team┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀

       ▄▄ website ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄   Build bench
       █ ▛▀▀▀▀▀▀▀▀▀▀▜  ▛▀▀▀▀▀▀▀▀▀▀▜ █   ▟▙ 1 claude-alpha-71c3ac
       █ ▌[yours!]  ▐  ▌          ▐ █   ▟▙ 2 grok-alpha-0f0f0f
       █ ▌ ◔        ▐  ▌ ×        ▐ █
       █ ▌▟█▙  ⏚!   ▐  ▌▟█▙       ▐ █
       █ ▌idle      ▐  ▌failed    ▐ █
       █ ▙▄▄▄▄▄▄▄▄▄▄▟  ▙▄▄▄▄▄▄▄▄▄▄▟ █
       █ opencode      ✖ codex      █   bench ▟▙ ▟▙
       ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
      codex-0b0b0b · ◆ asks you 12 s · "git push" · Internet …  ↓ 1 more row"
    `);
  });

  test('a wide screen shows the gate with the feed trough and the far island too', () => {
    const f = text(demoWorld(), view('ranch'), { w: 160, h: 30 });
    for (const w of ['Ranch gate', 'Shared notes 12 · 2 claimed', '▣ ns:build 3', 'claude 5h 35%', 'codex week 89%', 'Other computer', 'gpu-1', 'train', 'a job runs']) expect(f).toContain(w);
    expect(f).not.toContain('Saved sessions');
    expect(f).not.toContain('more row');
  });

  test('the island rests while no tool of it runs, and the stable shows with a saved session', () => {
    const w = demoWorld((d) => {
      const s = d.ps.sandboxes[0];
      if (s) s.devices = [];
      d.extra.horses = [{ id: 's1', horse: 'bay' }];
    });
    const f = text(w, view('ranch'), { w: 200, h: 30 });
    expect(f).toContain('gpu-1');
    expect(f).not.toContain('a job runs');
    expect(f).toContain('Saved sessions');
    expect(f).toContain('bay · 1');
  });

  test('every room, loft, yard, signpost, the bench, the gate and the island can be selected', () => {
    const d = draw(demoWorld(), view('ranch'), { w: 160, h: 30 });
    const ids = d.hits.map((h) => h.id);
    for (const id of [`room:${GROK}`, `room:${CODEX}`, `room:${CLAUDE}`, `room:${GEMINI}`, `room:${OPENCODE}`, 'room:codex-web-0d0d0d55', `loft:${ALPHA}`, `yard:${ALPHA}:team`, `sign:${ALPHA}`, 'bench', 'gate', 'island:gpu-1']) {
      expect(ids, id).toContain(id);
    }
    // The project without a loft has no ladder to select.
    expect(ids).not.toContain('loft:/src/website');
    for (const h of d.hits) {
      expect(h.x, h.id).toBeGreaterThanOrEqual(0);
      expect(h.x + h.w, h.id).toBeLessThanOrEqual(160);
      expect(h.y + h.h, h.id).toBeLessThanOrEqual(30);
    }
  });

  test('the selection scrolls the rows of the ranch', () => {
    const w = demoWorld();
    expect(text(w, view('ranch', { sel: `room:${CODEX}` }))).not.toContain('Ranch gate');
    const f = text(w, view('ranch', { sel: 'gate' }));
    expect(f).toContain('Ranch gate');
    expect(f).toContain('↑');
    expect(f).not.toContain('▄▄ alpha');
  });

  test('a house with more rooms than fit shows the ones that need you, and says how many are hidden', () => {
    const w = demoWorld((d) => {
      for (let i = 0; i < 6; i++) d.ps.sandboxes.push({ id: `claude-alpha-aaaaaa0${i}`, short: `aaaaa${i}`, harness: 'claude', project: '/src/alpha', network: 'egress', started: T - 5000 - i });
    });
    const d = draw(w, view('ranch'));
    const f = d.canvas.lines().join('\n');
    expect(f).toMatch(/\+\d more/);
    expect(d.hits.map((h) => h.id)).toContain(`room:${CODEX}`);
  });

  test('an empty ranch says so', () => {
    const w = demoWorld((d) => {
      d.ps.sandboxes = [];
      d.ps.ended = [];
      d.board.projects = [];
    });
    expect(text(w, view('ranch'))).toContain('The ranch is empty');
  });
});

describe('the building', () => {
  test('the rooms of one project, the ladder, the loft floor with the notes shelf, the tools of the hall', () => {
    const f = text(demoWorld(), view('building', { sel: `room:${CODEX}` }));
    expect(f.split('\n')[0]).toMatch(/^Ranch › alpha +◆ 1 asks you$/);
    for (const w of ['▲ Loft: private notes', 'notebooks on the shelf', '╫', '[Internet: a list of hosts]', '[Internet: model only]', '❄ Paused: ration is used up', '▐! Asks you: git push', '⏚ screenshot ══▶ your machine', '› codex-0b0b0b ◆ asks you · 12 s', 'grok-0f0f0f ❄ paused · 3 min']) {
      expect(f, w).toContain(w);
    }
    // Two rooms on a floor at 80 columns; the next floor is one step down.
    expect(f).toContain('↓ 1 more floor');
    const next = text(demoWorld(), view('building', { sel: `room:${GEMINI}` }));
    expect(next).toContain('› ↳ gemini-0a1a2a ● running');
    expect(next).toContain('⚑ Messages 2');
  });

  test('a project without a loft has no ladder, and an ended run has its light off', () => {
    const d = draw(demoWorld(), view('building', { house: '/src/website' }));
    const f = d.canvas.lines().join('\n');
    expect(f).toContain('No loft: this project keeps no private notes.');
    expect(f).not.toContain('╫');
    expect(f).toContain('the light is off');
    expect(f).toContain('[Internet: your network]');
    expect(d.hits.map((h) => h.kind)).not.toContain('loft');
  });

  test('a project that is gone', () => {
    expect(text(demoWorld(), view('building', { house: '/nowhere' }))).toContain('This project is gone from paddock');
  });
});

describe('the room', () => {
  const room = (id: string, o: Partial<View> = {}): View => view('room', { room: id, ...o });

  test('simple: the door with the request, the window, the mailbox, the copy on the desk, and the tool on your machine', () => {
    const f = text(demoWorld(), room(CODEX, { sel: 'door' }));
    expect(f).toMatchInlineSnapshot(`
      "Ranch › alpha › codex-0b0b0b                                        ◆ 1 asks you
           ▛▀▀╡▲ loft╞▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▜
           ▌                                      ▐
           ▌                         [model only] ▐╞═▶ Internet     model only
           ▌                                      ▐    Asks you     git push · 48 s
           ▌                                      ▐    Messages     none unread
           ▌                                      ▐    Agent        asks you · 12 s
           ▌                                      ▐    Working copy changes not known
           ▌                                      ▐    Shared yard  team · 2 rooms · …
           ▌                                      ▐    Loft         notes upstairs
           ▌                                      ▐    Tools on your machine
           ▌                                      ▐══▶ screenshot   your machine · ask
           ▌                                      ▐
           ▌   ◕ !                                ▐
           ▐! ▟█▙                     ▄▄▄ copy    ▐
       ⚐   ▌  ▀ ▀                     ▀▀▀         ▐
           ▌                                      ▐
           ▙▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▟


      Asks you: git push · 48 s"
    `);
  });

  test('Plumbing is off by default: only the tool on your machine shows. P shows every tool and its cable', () => {
    const w = demoWorld();
    const simple = text(w, room(CLAUDE));
    expect(simple).toContain('screenshot');
    expect(simple).toContain(LINK.host);
    for (const hidden of ['lint', 'train', 'cache', 'Shared notes', 'Borrowed folder', LINK.remote, LINK.inside]) expect(simple, hidden).not.toContain(hidden);
    expect(simple.split('\n')[0]).not.toContain('Plumbing');
    const f = text(w, room(CLAUDE, { plumbing: true }), { w: 100, h: 24 });
    expect(f.split('\n')[0]).toContain('Plumbing');
    const line = (name: string): string => f.split('\n').find((l) => l.includes(` ${name} `)) ?? '';
    expect(line('screenshot')).toMatch(/══▶ screenshot +your machine · ask/);
    expect(line('train')).toMatch(/≈≈▶ train +gpu-1 · on/);
    expect(line('lint')).toMatch(/──● lint +in the room/);
    expect(line('cache')).toMatch(/──● cache +in the room/);
    expect(f).toContain('Shared notes');
    expect(f).toContain('▦ notes');
    // The walls are glass.
    expect(f).toContain('┌┄┄');
    expect(simple).toContain('▛▀▀');
    // What paddock cannot list is named as that, not drawn as a crate.
    expect(f).toMatch(/Borrowed folder +paddock has no list yet/);
  });

  test('the plain words of the world, never the words of paddock', () => {
    const f = text(demoWorld(), room(CLAUDE, { plumbing: true }), { w: 120, h: 26 });
    for (const w of ['Internet', 'Messages', 'Working copy', 'Helpers', 'Shared yard', 'Timer', 'Loft', 'Tools', 'Shared notes', 'Borrowed folder']) expect(f, w).toContain(w);
    expect(f).toContain('3 changed files');
    expect(f).toContain('build 4 min left');
    expect(f).toContain('2 unread');
    for (const w of ['device', 'pasture', ' kv', 'spawn', 'sandbox', 'egress', 'allowlist']) expect(f.toLowerCase(), w).not.toContain(w);
  });

  test.each([
    ['none', 'closed', ''],
    ['api', 'model only', LINK.pipe],
    ['allowlist', 'a list of hosts', LINK.pipe],
    ['egress', 'open', LINK.open],
    ['host', 'your network', LINK.host],
    ['ask', 'asks each time', LINK.pipe],
    ['vpn', 'not known', ''],
  ])('the window in network mode %s: "%s"', (mode, word, link) => {
    const w = demoWorld((d) => {
      const s = d.ps.sandboxes.find((x) => x.id === GEMINI);
      if (s) s.network = mode;
      if (d.extra.devices) d.extra.devices.devices = [];
      d.pending = [];
    });
    const f = text(w, room(GEMINI));
    const line = f.split('\n').find((l) => l.includes('Internet')) ?? '';
    expect(line).toContain(`Internet     ${word}`);
    expect(line.trimEnd().endsWith(word)).toBe(true);
    expect(f).toContain(`[${word}]`);
    for (const l of Object.values(LINK)) expect(line.includes(l), l).toBe(l === link);
    // Only the door to your own network is amber.
    expect(amber(w, room(GEMINI))).toBe(mode === 'host');
  });

  test('a paused room keeps its character and says why; an ended room is dark', () => {
    const f = text(demoWorld(), room(GROK), { w: 100 });
    expect(f).toContain('❄ paused');
    expect(f).toMatch(/Paused +ration is used up · 3 min/);
    const ended = text(demoWorld(), view('room', { house: '/src/website', room: 'codex-web-0d0d0d55' }));
    expect(ended).toContain('the light is off');
    expect(ended).not.toContain('Internet');
    expect(ended).toMatch(/Agent +failed/);
  });

  test('a helper names the run that started it, and that run counts its helpers', () => {
    expect(text(demoWorld(), room(GEMINI), { w: 100 })).toMatch(/Helper +of claude-71c3ac · T012/);
    expect(text(demoWorld(), room(CLAUDE))).toMatch(/Helpers +1 on the path/);
  });

  test('the lines of the panel can be selected; the header cannot', () => {
    const d = draw(demoWorld(), room(CLAUDE, { plumbing: true }), { w: 100, h: 26 });
    expect(d.hits.map((h) => h.id)).toEqual(['window', 'door', 'mail', 'self', 'copy', 'helper', 'yard', 'timer:build', `loft:${ALPHA}`, 'tool:screenshot', 'tool:train', 'tool:cache', 'tool:lint', 'notes', 'borrow']);
    const lines = panelLines(demoWorld(), demoWorld().houses[0] as never, demoWorld().houses[0]?.rooms[2] as never, true, T);
    expect(lines.filter((l) => l.header).map((l) => l.label)).toEqual(['Tools']);
  });

  test('a low screen cuts the panel and says so', () => {
    const f = text(demoWorld(), room(CLAUDE, { plumbing: true }), { w: 80, h: 14 });
    expect(f).toMatch(/\+\d+ more in a taller window/);
  });

  test('a run that is gone', () => {
    expect(text(demoWorld(), room('nobody'))).toContain('This run is gone from paddock');
  });
});

describe('the loft', () => {
  test('one loft per project: the ladder, the shelf, and what paddock does not list yet', () => {
    const d = draw(demoWorld(), view('loft'));
    const f = d.canvas.lines().join('\n');
    expect(f.split('\n')[0]).toContain('Ranch › alpha › Loft');
    for (const w of ['Loft: private notes', 'Notes, newest first', 'Folder  /data/loft/abc/tree/.paddock', '╫']) expect(f, w).toContain(w);
    expect(d.hits.map((h) => h.id)).toEqual(['note:log', 'note:folder']);
    expect(text(demoWorld(), view('loft', { house: '/src/website' }))).toContain('This project has no loft');
  });
});

describe('amber', () => {
  const calm = (): World =>
    demoWorld((d) => {
      d.pending = [];
      d.board.approvals_waiting = 0;
      if (d.board.projects[0]?.tickets?.[0]) d.board.projects[0].tickets[0].state = 'working';
      if (d.extra.devices) d.extra.devices.devices = d.extra.devices.devices.filter((x) => x.run?.run !== 'host');
      const s = d.ps.sandboxes.find((x) => x.id === OPENCODE);
      if (s) s.network = 'egress';
    });

  test('nothing waits and nothing touches your machine: no amber in any picture', () => {
    const w = calm();
    for (const plumbing of [false, true]) {
      expect(amber(w, view('ranch', { plumbing }), { w: 200, h: 40 })).toBe(false);
      expect(amber(w, view('building', { plumbing }), { w: 200, h: 40 })).toBe(false);
      for (const id of [CLAUDE, CODEX, GEMINI, GROK]) expect(amber(w, view('room', { room: id, plumbing }), { w: 120, h: 30 }), id).toBe(false);
      expect(amber(w, view('loft', { plumbing }))).toBe(false);
    }
  });

  test('a request is amber in every picture', () => {
    const w = demoWorld((d) => {
      if (d.extra.devices) d.extra.devices.devices = [];
    });
    expect(amber(w, view('ranch'))).toBe(true);
    expect(amber(w, view('building'))).toBe(true);
    expect(amber(w, view('room', { room: CODEX }))).toBe(true);
    // The room next door has no amber of its own, only the count in the top line.
    const d = draw(w, view('room', { room: CLAUDE }), { mode: 'truecolor' });
    const rows = d.canvas.rows().slice(1);
    for (const a of AMBERS) expect(rows.some((r) => r.st.some((s) => s.includes(sgr(a))))).toBe(false);
  });

  test('a tool on your machine is amber also in the simple view, and a tool that only asks is not', () => {
    const host = demoWorld((d) => {
      d.pending = [];
    });
    expect(has(host, view('room', { room: CLAUDE }), AMBER)).toBe(true);
    expect(has(host, view('building', { sel: `room:${CLAUDE}` }), AMBER)).toBe(true);
    expect(has(host, view('ranch'), AMBER, { w: 160, h: 30 })).toBe(true);
    const asks = calm();
    asks.houses[0]?.rooms.forEach((r) => r.tools.forEach((t) => (t.ask = true)));
    expect(amber(asks, view('room', { room: CLAUDE, plumbing: true }), { w: 120, h: 30 })).toBe(false);
  });
});

describe('the forms of the picture', () => {
  const views: View[] = [view('ranch'), view('building'), view('room', { room: CLAUDE, plumbing: true }), view('room', { room: CODEX }), view('loft')];

  test.each([
    [80, 21],
    [100, 27],
    [200, 57],
  ])('%i x %i: every picture has exactly its size, in art and in text', (w, h) => {
    for (const v of views) {
      for (const mode of ['truecolor', '256', 'none'] as ArtMode[]) {
        const rows = draw(demoWorld(), v, { w, h, mode }).canvas.rows();
        expect(rows).toHaveLength(h);
        for (const r of rows) {
          expect(r.ch).toHaveLength(w);
          expect(cells(r.ch.join(''))).toBe(w);
        }
      }
    }
  });

  test('art: half blocks in 24-bit colour, or in the 256 colours', () => {
    const t = draw(demoWorld(), view('ranch'), { mode: 'truecolor' }).canvas.rows().map(rowText).join('\n');
    expect(t).toContain('▀');
    expect(t).toMatch(/\u001b\[[\d;]*38;2;\d+;\d+;\d+/);
    const c = draw(demoWorld(), view('ranch'), { mode: '256' }).canvas.rows().map(rowText).join('\n');
    expect(c).toMatch(/\u001b\[[\d;]*38;5;\d+/);
    expect(c).not.toMatch(/[34]8;2;/);
  });

  test('NO_COLOR: a clean text version, with no colour code in any picture', () => {
    for (const v of views) {
      const t = draw(demoWorld(), v, { mode: 'none' }).canvas.rows().map(rowText).join('\n');
      expect(t).not.toMatch(/\u001b\[[\d;]*(3\d|4\d|9\d|10\d)[;m]/);
      // Only bold, dim, underline and inverse.
      for (const m of t.matchAll(/\u001b\[([\d;]*)m/g)) expect(m[1]).toMatch(/^0?(;[1247])*$/);
    }
  });

  test('--plain: no escape sequence at all', () => {
    for (const v of views) expect(draw(demoWorld(), v, { mode: 'none', plain: true }).canvas.rows().map(rowText).join('\n')).not.toContain('\u001b');
  });

  test('a light terminal: amber and the accent in their darker shades where they lie on its background', () => {
    const w = demoWorld();
    const top = (light: boolean): string => drawWorld(w, { w: 80, h: 21, mode: 'truecolor', light, tick: 0, motion: new Motion(), view: view('ranch'), t: T }).canvas.rows()[0]?.st.join('|') ?? '';
    expect(top(false)).toContain(sgr(AMBER));
    expect(top(true)).not.toContain(sgr(AMBER));
    expect(top(true)).toContain(sgr(AMBER_DARK));
  });

  test('a screen that is too small says what the world needs', () => {
    const d = draw(demoWorld(), view('ranch'), { w: 50, h: 10 });
    expect(d.canvas.lines()[0]).toContain('The world needs 60 columns and 11 rows');
    expect(d.hits).toEqual([]);
  });

  test('text from a sandbox cannot reach the terminal as an escape sequence', () => {
    const w = demoWorld((d) => {
      const p = d.pending[0];
      if (p) p.summary = 'git\u001b[2J push\u001b]0;owned\u0007 ‮x';
      const s = d.ps.sandboxes.find((x) => x.id === CODEX);
      if (s) s.activity = 'a\u009b2Jb';
    });
    for (const v of [view('ranch', { sel: `room:${CODEX}` }), view('building', { sel: `room:${CODEX}` }), view('room', { room: CODEX, sel: 'door' })]) {
      const rows = draw(w, v, { mode: 'none', plain: true, w: 200, h: 30 }).canvas.rows();
      const t = rows.map((r) => r.ch.join('')).join('\n');
      expect(t).not.toMatch(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f‮]/);
      expect(t).toContain('git push x');
    }
  });
});

describe('the top line and the selection', () => {
  test('the breadcrumb names each stop of the camera', () => {
    const w = demoWorld();
    expect(breadcrumb(w, view('ranch'))).toBe('Ranch');
    expect(breadcrumb(w, view('building'))).toBe('Ranch › alpha');
    expect(breadcrumb(w, view('room', { room: CLAUDE }))).toBe('Ranch › alpha › claude-71c3ac');
    expect(breadcrumb(w, view('loft'))).toBe('Ranch › alpha › Loft');
  });

  test('one line of plain words on what is selected', () => {
    const w = demoWorld();
    const of = (sel: string, level: Level = 'ranch', room = ''): string => infoLine(w, view(level, { sel, room }), T);
    expect(of(`room:${CLAUDE}`)).toBe('claude-71c3ac · ● running 16 min · Internet open · on your machine: screenshot · yard team · T-102 · T-102: code and 14 tests pass');
    expect(of(`yard:${ALPHA}:team`)).toBe('Shared yard team: 2 rooms share one folder and 4 notes, ports 8080');
    expect(of('bench')).toBe('Build bench: 1 claude-alpha-71c3ac, 2 grok-alpha-0f0f0f');
    expect(of('island:gpu-1')).toBe('Other computer gpu-1: tools train run there.');
    expect(of(`sign:${ALPHA}`)).toBe('Tickets of alpha: T-101 waiting, T-102 working, T-103 open');
    expect(of('tool:screenshot', 'room', CLAUDE)).toBe('screenshot: your machine · ask');
    expect(of('window', 'room', CODEX)).toBe('Internet: model only');
  });

  test('the arrow keys go to the nearest thing in their direction', () => {
    const hits: Hit[] = [
      { id: 'a', kind: 'room', x: 0, y: 0, w: 10, h: 5 },
      { id: 'b', kind: 'room', x: 14, y: 0, w: 10, h: 5 },
      { id: 'c', kind: 'room', x: 40, y: 0, w: 10, h: 5 },
      { id: 'd', kind: 'room', x: 2, y: 10, w: 10, h: 5 },
      { id: 'e', kind: 'bench', x: 30, y: 10, w: 10, h: 5 },
    ];
    expect(moveSelection(hits, 'a', 1, 0)).toBe('b');
    expect(moveSelection(hits, 'b', 1, 0)).toBe('c');
    expect(moveSelection(hits, 'c', 1, 0)).toBe('c');
    expect(moveSelection(hits, 'b', -1, 0)).toBe('a');
    expect(moveSelection(hits, 'a', 0, 1)).toBe('d');
    expect(moveSelection(hits, 'c', 0, 1)).toBe('e');
    expect(moveSelection(hits, 'e', 0, -1)).toBe('c');
    expect(moveSelection(hits, 'd', 0, -1)).toBe('a');
    // A selection that is not in the picture: the first thing.
    expect(moveSelection(hits, 'gone', 1, 0)).toBe('a');
    expect(moveSelection([], 'gone', 1, 0)).toBe('gone');
  });
});

describe('the Inspector', () => {
  const hitOf = (w: World, v: View, id: string, o: Parameters<typeof draw>[2] = {}): Hit | undefined => draw(w, v, o).hits.find((h) => h.id === id);

  test('the paddock word and the exact command behind each thing of a room', () => {
    const w = demoWorld();
    const v = view('room', { room: CODEX, plumbing: true });
    const of = (id: string) => inspect(w, hitOf(w, v, id, { w: 100, h: 26 }));
    expect(of('door')).toMatchObject({ label: 'Asks you', word: 'approval request', commands: ['paddock approvals --json', 'paddock approvals answer r-1 once|session|deny'] });
    expect(of('door')?.json).toMatchObject({ id: 'r-1', summary: 'git push', capability: 'exec' });
    expect(of('window')).toMatchObject({ label: 'Internet', word: 'network mode api', commands: ['paddock ps --json'], json: { network: 'api' } });
    expect(of('mail')).toMatchObject({ word: 'mail', commands: ['paddock mail ls 0b0b0b --json', 'paddock mail send 0b0b0b -'] });
    expect(of('self')).toMatchObject({ commands: ['paddock freeze 0b0b0b [--tree]', 'paddock thaw 0b0b0b [--tree]', 'paddock ps --json'] });
    expect(of('copy')).toMatchObject({ word: 'files of the sandbox', commands: ['paddock files 0b0b0b --json'] });
    expect(of(`loft:${ALPHA}`)).toMatchObject({ word: 'loft', commands: ['paddock loft status --dir /src/alpha', 'paddock loft log --dir /src/alpha', 'paddock loft diff --dir /src/alpha'], json: { loft: '/data/loft/abc/tree/.paddock' } });
    expect(of('tool:screenshot')).toMatchObject({ label: 'Tool', word: 'device screenshot (run=host, scope=run, mode=ask)', commands: ['paddock device ls --json', 'paddock device stats screenshot --json', 'paddock device ledger --json'] });
    expect(of('tool:screenshot')?.json).toMatchObject({ name: 'screenshot', run: { run: 'host' }, mode: 'ask' });
    expect(of('tool:cache')?.word).toBe('device cache (run=sandbox, scope=shared, mode=allow)');
    expect(of('notes')).toMatchObject({ word: 'kv', commands: ['paddock kv list --json --sandbox 0b0b0b', 'paddock kv list --json --pasture team', 'paddock kv list --json --user', 'paddock kv overview --json'] });
    expect(of('yard')).toMatchObject({ word: 'pasture team', commands: ['paddock pasture list --json', 'paddock kv list --json --pasture team', 'paddock kv overview --json'] });
    expect(of('borrow')?.gap).toBe('paddock has no command that lists the borrowed folders of a run.');
  });

  test('the things of the ranch', () => {
    const w = demoWorld();
    const v = view('ranch');
    const of = (id: string) => inspect(w, hitOf(w, v, id, { w: 160, h: 30 }));
    expect(of(`room:${GEMINI}`)).toMatchObject({ label: 'Helper room', word: 'run gemini-0a1a2a in a sandbox (bwrap)', commands: ['paddock ps --json', 'paddock view 0a1a2a', 'paddock stop 0a1a2a'] });
    expect(of(`room:${GEMINI}`)?.json).toMatchObject({ id: GEMINI, parent: CLAUDE, spec_task: 'T012', network: 'none' });
    expect(of('bench')).toMatchObject({ word: 'compile slots (kv leases compile-slot-N of the user scope)', commands: ['paddock board --json', 'paddock kv list --json --user lease:'] });
    expect(of('island:gpu-1')).toMatchObject({ label: 'Other computer: gpu-1', json: { name: 'gpu-1', target: 'ssh://gpu-1', tools: ['train'] } });
    expect(of('gate')?.commands).toEqual(['paddock kv overview --json', 'paddock kv list --json --user', 'paddock forage --json']);
    expect(of(`sign:${ALPHA}`)?.commands[0]).toBe('paddock board --json');
    expect(inspect(w, undefined)).toBeNull();
  });

  test('the actions of a thing, and none that changes something with --readonly', () => {
    const w = demoWorld();
    const room = hitOf(w, view('ranch'), `room:${GROK}`);
    expect(inspect(w, room)?.actions.map((a) => `${a.key} ${a.label}`)).toEqual(['↵ zoom in', 'z resume (thaw)', 'w write to the agent', 's stop the run', 'o open its live view']);
    expect(inspect(w, room, true)?.actions.map((a) => a.key)).toEqual(['↵', 'o']);
    const door = hitOf(w, view('room', { room: CODEX }), 'door');
    expect(inspect(w, door)?.actions.map((a) => a.key)).toEqual(['↵']);
    expect(inspect(w, door, true)?.actions).toEqual([]);
  });

  test('the lines of the panel: the label, the paddock word, the command, the JSON, the gap, the actions', () => {
    const w = demoWorld();
    const i = inspect(w, hitOf(w, view('room', { room: CODEX }), 'window'));
    const lines = inspectorLines(i as never, 34).map((l) => l.text);
    expect(lines).toEqual(['Internet', 'paddock: network mode api', 'One pipe: only the service of its', 'model.', '', '$ paddock ps --json', '', 'Not yet: ps does not list the', 'allowed hosts of a run.', '', '{', ' "network": "api"', '}']);
    for (const l of lines) expect(cells(l)).toBeLessThanOrEqual(34);
    expect(wrap('aaaa bbbb cccccccccc d', 6)).toEqual(['aaaa', 'bbbb', 'cccccc', 'cccc d']);
  });
});
