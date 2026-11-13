// The Inspector: for the selected thing of the world, the word that
// paddock uses for it, the exact commands behind it, the JSON that the
// picture was drawn from, and what you can do. The world speaks plain
// words; this panel is where the paddock words are.

import { cells, cut, oneLine } from '../clean.js';
import { Canvas, type TextStyle } from './canvas.js';
import { t } from '../i18n/index.js';
import { netSentence, type House, type Room, type World } from './model.js';
import * as px from './pixels.js';
import type { Hit } from './scene.js';

export interface Inspection {
  /** The plain word of the world. */
  label: string;
  /** The word of paddock. */
  word: string;
  /** What the thing is, in a sentence or two. */
  about: string[];
  /** The commands behind it, the first is the one that the picture reads. */
  commands: string[];
  /** The JSON that the picture was drawn from. */
  json?: unknown;
  /** What paddock does not report yet. */
  gap?: string;
  /** The keys that act on it. */
  actions: { key: string; label: string; changes?: boolean }[];
}

function roomOf(world: World, id: string | undefined): { h: House; r: Room } | undefined {
  return world.houses.flatMap((h) => h.rooms.map((r) => ({ h, r }))).find((e) => e.r.id === id);
}

/** A value without its long or empty parts: the Inspector shows a few lines of it. */
function brief(v: unknown): unknown {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return v;
  const out: Record<string, unknown> = {};
  for (const [k, x] of Object.entries(v)) {
    if (x === null || x === undefined || x === '' || (Array.isArray(x) && x.length === 0)) continue;
    out[k] = x;
  }
  return out;
}

export function inspect(world: World, hit: Hit | undefined, readonly = false): Inspection | null {
  if (!hit) return null;
  const e = roomOf(world, hit.room);
  const id = e?.r.short ?? 'ID';
  const act = (list: Inspection['actions']): Inspection['actions'] => list.filter((a) => !(readonly && a.changes));
  switch (hit.kind) {
    case 'room': {
      if (!e) return null;
      const r = e.r;
      return {
        label: r.parent ? t('world.inspect.helperRoom') : t('world.inspect.room'),
        word: t('world.inspect.roomWord', { name: r.name, backend: r.run.backend || t('world.inspect.backendUnknown') }),
        about: [t('world.inspect.roomAbout')],
        commands: ['paddock ps --json', `paddock view ${id}`, `paddock stop ${id}`],
        json: brief(r.ps ?? { id: r.id, state: r.run.state, ended: r.run.since, last: r.run.last }),
        actions: act([
          { key: '↵', label: t('world.inspect.zoomIn') },
          { key: 'z', label: r.run.frozen ? t('world.inspect.resume') : t('world.inspect.pause'), changes: true },
          { key: 'w', label: t('world.inspect.writeAgent'), changes: true },
          { key: 's', label: t('world.inspect.stopRun'), changes: true },
          { key: 'o', label: t('world.inspect.openView') },
        ]),
      };
    }
    case 'door': {
      const p = e?.r.asks[0];
      return {
        label: t('world.inspect.asks'),
        word: t('world.inspect.asksWord'),
        about: [p ? t('world.inspect.asksWaits') : t('world.inspect.asksNone')],
        commands: ['paddock approvals --json', `paddock approvals answer ${p?.id ?? 'REQUEST'} once|session|deny`],
        json: p ? brief(p) : undefined,
        actions: act(p ? [{ key: '↵', label: t('world.inspect.openCard'), changes: true }] : []),
      };
    }
    case 'window': {
      const r = e?.r;
      return {
        label: t('world.inspect.internet'),
        word: t('world.inspect.netWord', { mode: r?.netRaw || '?' }),
        about: [r ? netSentence(r.net) : ''],
        commands: ['paddock ps --json'],
        json: r ? { network: r.netRaw } : undefined,
        gap: t('world.inspect.netGap'),
        actions: [],
      };
    }
    case 'mail':
      return {
        label: t('world.inspect.messages'),
        word: 'mail',
        about: [t('world.inspect.mailAbout')],
        commands: [`paddock mail ls ${id} --json`, `paddock mail send ${id} -`],
        json: e ? { unread_up: e.r.unread } : undefined,
        actions: act([
          { key: '↵', label: t('world.inspect.readThread') },
          { key: 'w', label: t('world.inspect.write'), changes: true },
        ]),
      };
    case 'self': {
      const r = e?.r;
      return {
        label: r?.run.frozen ? t('world.inspect.paused') : t('world.inspect.agent'),
        word: r?.run.frozen ? t('world.inspect.frozenWord') : t('world.inspect.stateWord', { state: r?.run.state ?? '?' }),
        about: [r?.run.frozen ? t('world.inspect.frozenAbout') : t('world.inspect.pauseAbout')],
        commands: [`paddock freeze ${id} [--tree]`, `paddock thaw ${id} [--tree]`, 'paddock ps --json'],
        json: r?.ps ? brief({ frozen_since: r.ps.frozen_since, frozen_reason: r.ps.frozen_reason, idle_since: r.ps.idle_since, activity: r.ps.activity, stuck: r.ps.stuck }) : undefined,
        actions: act(r?.run.live ? [{ key: 'z', label: r.run.frozen ? t('world.inspect.resume') : t('world.inspect.pause'), changes: true }] : []),
      };
    }
    case 'copy': {
      const r = e?.r;
      return {
        label: t('world.inspect.copy'),
        word: t('world.inspect.copyWord'),
        about: [r && r.changedFiles.length > 0 ? cut(r.changedFiles.join(', '), 160) : t('world.inspect.copyAbout')],
        commands: [`paddock files ${id} --json`],
        json: r ? { changed: r.changed === null ? t('world.inspect.notKnown') : r.changed } : undefined,
        gap: t('world.inspect.copyGap'),
        actions: [],
      };
    }
    case 'loft': {
      const h = world.houses.find((x) => x.key === hit.house);
      return {
        label: t('world.inspect.loft'),
        word: 'loft',
        about: [t('world.inspect.loftAbout')],
        commands: [`paddock loft status --dir ${h?.path || 'PROJECT'}`, `paddock loft log --dir ${h?.path || 'PROJECT'}`, `paddock loft diff --dir ${h?.path || 'PROJECT'}`],
        json: h ? { loft: h.loft } : undefined,
        gap: t('world.inspect.loftGap'),
        actions: [{ key: '↵', label: t('world.inspect.upstairs') }],
      };
    }
    case 'note': {
      const h = world.houses.find((x) => x.key === hit.house);
      return {
        label: t('world.inspect.notesUp'),
        word: 'loft log',
        about: [t('world.inspect.notesAbout')],
        commands: [`paddock loft log --dir ${h?.path || 'PROJECT'}`, `paddock loft status --dir ${h?.path || 'PROJECT'}`],
        json: h ? { loft: h.loft } : undefined,
        gap: t('world.inspect.notesGap'),
        actions: [],
      };
    }
    case 'tool': {
      const tool = e?.r.tools.find((x) => x.name === hit.name) ?? world.houses.flatMap((h) => h.hall).find((x) => x.name === hit.name);
      const where = tool?.run === 'host' ? t('world.inspect.toolHost') : tool?.run === 'remote' ? t('world.inspect.toolRemote', { peer: tool.peer }) : tool?.run === 'sandbox' ? t('world.inspect.toolSandbox') : '';
      return {
        label: t('world.inspect.tool'),
        word: `device ${hit.name ?? ''} (run=${tool?.run ?? '?'}, scope=${tool?.shared ? 'shared' : 'run'}, mode=${tool?.ask ? 'ask' : 'allow'})`,
        about: [where, tool?.ask ? t('world.inspect.toolAsk') : t('world.inspect.toolAllow')].filter((x) => x.length > 0),
        commands: ['paddock device ls --json', `paddock device stats ${hit.name ?? 'NAME'} --json`, 'paddock device ledger --json'],
        json: tool ? brief(tool.def) : undefined,
        gap: t('world.inspect.toolGap'),
        actions: [],
      };
    }
    case 'notes':
      return {
        label: t('world.inspect.kv'),
        word: 'kv',
        about: [t('world.inspect.kvAbout')],
        commands: [`paddock kv list --json --sandbox ${id}`, `paddock kv list --json --pasture ${e?.r.yard || 'NAME'}`, 'paddock kv list --json --user', 'paddock kv overview --json'],
        gap: t('world.inspect.kvGap'),
        actions: [],
      };
    case 'borrow':
      return {
        label: t('world.inspect.borrow'),
        word: 'borrow',
        about: [t('world.inspect.borrowAbout')],
        commands: ['paddock-host borrow FOLDER --reason TEXT', 'paddock-host return NAME'],
        gap: t('world.inspect.borrowGap'),
        actions: [],
      };
    case 'yard': {
      const y = world.houses.flatMap((h) => h.yards).find((x) => x.name === hit.name && (!hit.house || x.project === world.houses.find((h) => h.key === hit.house)?.path));
      return {
        label: t('world.inspect.yard'),
        word: `pasture ${hit.name ?? ''}`,
        about: [t('world.inspect.yardAbout')],
        commands: ['paddock pasture list --json', `paddock kv list --json --pasture ${hit.name ?? 'NAME'}`, 'paddock kv overview --json'],
        json: y ? brief(y) : undefined,
        actions: [],
      };
    }
    case 'helper': {
      const r = e?.r;
      return {
        label: t('world.inspect.helper'),
        word: 'spawn',
        about: [r?.parent ? t('world.inspect.helperChild') : t('world.inspect.helperParent')],
        commands: ['paddock ps --json', `paddock mail ls ${id} --json`],
        json: r ? brief({ parent: r.parent, spawn_id: r.ps?.spawn_id, spec_task: r.specTask, children: r.children }) : undefined,
        actions: [],
      };
    }
    case 'timer':
      return {
        label: t('world.inspect.timer'),
        word: 'timer',
        about: [t('world.inspect.timerAbout')],
        commands: ['paddock timer list --json'],
        json: e?.r.timers.find((t) => (t.name ?? '') === (hit.name ?? '')),
        actions: [],
      };
    case 'bench':
      return {
        label: t('world.inspect.bench'),
        word: t('world.inspect.benchWord'),
        about: [t('world.inspect.benchAbout')],
        commands: ['paddock board --json', 'paddock kv list --json --user lease:'],
        json: world.bench.seats,
        gap: t('world.inspect.benchGap'),
        actions: [],
      };
    case 'island': {
      const i = world.islands.find((x) => x.name === hit.name);
      return {
        label: t('world.inspect.island', { name: hit.name ?? '' }),
        word: t('world.inspect.islandWord'),
        about: [t('world.inspect.islandAbout')],
        commands: ['paddock device ls --json', 'paddock exclave cache ls'],
        json: i,
        gap: t('world.inspect.islandGap'),
        actions: [],
      };
    }
    case 'gate':
      return {
        label: t('world.inspect.gate'),
        word: t('world.inspect.gateWord'),
        about: [t('world.inspect.gateAbout')],
        commands: ['paddock kv overview --json', 'paddock kv list --json --user', 'paddock forage --json'],
        json: brief({ keys: world.gate.keys, leases: world.gate.leases, namespaces: world.gate.cabinets, forage: world.gate.forage }),
        actions: [],
      };
    case 'stable':
      return {
        label: t('world.inspect.stable'),
        word: 'horse',
        about: [t('world.inspect.stableAbout')],
        commands: ['paddock horse list --json', 'paddock horse status --json'],
        json: world.stable?.stalls.map((s) => ({ horse: s.horse, sessions: s.sessions.length })),
        actions: [],
      };
    case 'sign': {
      const h = world.houses.find((x) => x.key === hit.house);
      return {
        label: t('world.inspect.sign'),
        word: t('world.inspect.signWord'),
        about: [t('world.inspect.signAbout')],
        commands: ['paddock board --json', 'paddock board --watch --json'],
        json: h?.tickets,
        actions: [{ key: 'L', label: t('world.inspect.boardList') }],
      };
    }
  }
}

/** Break a text into lines of at most `width` cells, at spaces where it can. */
export function wrap(text: string, width: number): string[] {
  if (width <= 0) return [];
  const out: string[] = [];
  for (const para of text.split('\n')) {
    let line = '';
    for (const word of para.split(' ')) {
      let w = word;
      while (cells(w) > width) {
        if (line) {
          out.push(line);
          line = '';
        }
        let part = '';
        for (const ch of w) {
          if (cells(part + ch) > width) break;
          part += ch;
        }
        // A character wider than the line still takes a line of its own: no endless loop.
        if (part === '') part = Array.from(w)[0] ?? '';
        out.push(part);
        w = w.slice(part.length);
      }
      if (line === '') line = w;
      else if (cells(line) + 1 + cells(w) <= width) line += ' ' + w;
      else {
        out.push(line);
        line = w;
      }
    }
    out.push(line);
  }
  return out;
}

/** The lines of the Inspector, each with its style: for the canvas, and for tests. */
export function inspectorLines(i: Inspection, width: number): { text: string; style: TextStyle }[] {
  const out: { text: string; style: TextStyle }[] = [];
  const add = (text: string, style: TextStyle = {}): void => {
    for (const l of wrap(text, width)) out.push({ text: l, style });
  };
  add(i.label, { bold: true });
  add(t('world.inspect.paddockWord', { word: i.word }), {});
  for (const a of i.about) add(a, { dim: true });
  out.push({ text: '', style: {} });
  i.commands.forEach((c, n) => add(`${n === 0 ? '$' : ' '} ${c}`, { bold: n === 0 }));
  if (i.actions.length > 0) {
    out.push({ text: '', style: {} });
    for (const a of i.actions) add(`${a.key}  ${a.label}`, { bold: true });
  }
  if (i.gap) {
    out.push({ text: '', style: {} });
    add(t('world.inspect.notYet', { gap: i.gap }), { dim: true });
  }
  // The JSON comes last: it is the longest part, and a low screen cuts it first.
  if (i.json !== undefined) {
    out.push({ text: '', style: {} });
    const text = JSON.stringify(i.json, null, 1) ?? '';
    // The indent of the JSON stays; the text of each line is cleaned.
    for (const l of text.split('\n').slice(0, 40)) out.push({ text: cut((/^ */.exec(l)?.[0] ?? '') + oneLine(l), width), style: { dim: true } });
  }
  return out;
}

/** The Inspector as a box of text on the canvas, over what is there. */
export function drawInspector(c: Canvas, i: Inspection | null, x: number, y: number, w: number, h: number): void {
  if (w < 12 || h < 4) return;
  // The box takes its place from the picture: no pixel shows through.
  c.fill(x, y * 2, w, h * 2, null);
  c.clearText(x, y, w, h);
  const line: TextStyle = c.art ? { fg: px.accentText(c.light) } : {};
  const title = ` ${t('world.inspect.title')} `;
  c.text(x, y, '╭' + '─'.repeat(w - 2) + '╮', line);
  c.text(x + 2, y, cut(title, w - 4), { ...line, bold: true });
  for (let r = 1; r < h - 1; r++) {
    c.text(x, y + r, '│' + ' '.repeat(w - 2) + '│', line);
    c.text(x + 1, y + r, ' '.repeat(w - 2), {});
  }
  c.text(x, y + h - 1, '╰' + '─'.repeat(w - 2) + '╯', line);
  const lines = i ? inspectorLines(i, w - 4) : [{ text: t('world.inspect.select'), style: { dim: true } as TextStyle }];
  const room = h - 2;
  lines.slice(0, room).forEach((l, n) => {
    if (n === room - 1 && lines.length > room) c.text(x + 2, y + 1 + n, cut(t('world.inspect.moreLines', { n: lines.length - room + 1 }), w - 4), { dim: true });
    else c.text(x + 2, y + 1 + n, l.text, l.style);
  });
}
