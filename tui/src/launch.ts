// What the home screen needs to start an agent: the shapes of the JSON it
// reads, the words of each choice, and the arguments of `paddock run`.
// Nothing here draws.

import { isAbsolute, resolve } from 'node:path';
import { cells, charCells, oneLine } from './clean.js';
import { t, type Key } from './i18n/index.js';

/** One entry of `paddock recent --json`. */
export interface Recent {
  project: string;
  harness?: string;
  last?: number;
}

/**
 * `folder_check` of `paddock explain --json`: what the folder guard says
 * about a project folder. A paddock without the guard does not send it.
 */
export interface FolderCheck {
  folder?: string;
  /** ok, confirmed (a wide folder that the user allowed), confirm (a wide folder that needs a yes) or refuse. */
  level?: string;
  reason?: string;
  fix?: string;
  confirmed_by?: string;
  message?: string;
}

/** What `paddock explain HARNESS FOLDER --json` says, as far as the home screen reads it. */
export interface Explain {
  network?: string;
  approvals?: string;
  folder_check?: FolderCheck;
  /** The command failed: its error line. With a refused folder, `folder_check` is there too. */
  error?: string;
}

/** The handover of the terminal to a paddock command: the interface leaves, the command runs, the home screen comes back. */
export interface Job {
  /** The arguments of paddock. */
  args: string[];
  /** A few words for the lines before and after: "claude in ~/code/app". */
  title: string;
  /** Wait for Enter before the home screen comes back: always (the output is the result), or only after a failure. */
  pause: 'always' | 'on-error';
  /** Another program than paddock: the editor of the user. */
  bin?: string;
  /** Where the interface opens again after the command: the files of a run. Default: the home screen. */
  resume?: { files: string };
}

export interface NetworkLevel {
  /** The value of --network, or '' to leave it to the configuration. */
  id: string;
  label: string;
  words: string;
  /** A wide level: shown as a warning. */
  wide?: boolean;
}

/** The network levels of a new agent, in plain words. The allow list needs domains, so it stays in paddock.toml. */
function level(id: string, label: Key, words: Key, wide?: boolean): NetworkLevel {
  return {
    id,
    get label() {
      return t(label);
    },
    get words() {
      return t(words);
    },
    ...(wide ? { wide } : {}),
  };
}

export const NETWORKS: NetworkLevel[] = [
  level('', 'launch.net.default', 'launch.net.defaultWords'),
  level('api', 'launch.net.api', 'launch.net.apiWords'),
  level('egress', 'launch.net.egress', 'launch.net.egressWords'),
  level('none', 'launch.net.none', 'launch.net.noneWords'),
  level('host', 'launch.net.host', 'launch.net.hostWords', true),
];

/** The plain words of a network level that paddock reports. */
export function networkWord(id: string | undefined): string {
  if (id === 'allowlist') return t('launch.net.allowlist');
  return NETWORKS.find((n) => n.id !== '' && n.id === id)?.label ?? '';
}

export const APPROVALS_WORDS: Record<'auto' | 'manual', string> = {
  get auto() {
    return t('launch.approvals.auto');
  },
  get manual() {
    return t('launch.approvals.manual');
  },
};

export interface RunChoice {
  agent: string;
  folder: string;
  /** '' leaves the level to the configuration. */
  network: string;
  /** null leaves the mode to the configuration. */
  approvals: 'auto' | 'manual' | null;
  /** Work in a new git worktree on its own branch. */
  branch: boolean;
  /** The user said yes to a wide folder. */
  wide: boolean;
}

/** The arguments of the `paddock run` that Start runs in the terminal. */
export function runArgs(c: RunChoice): string[] {
  const a = ['run', c.agent, c.folder];
  if (c.network) a.push('--network', c.network);
  if (c.approvals) a.push(`--${c.approvals}`);
  if (c.branch) a.push('--worktree');
  if (c.wide) a.push('--yes-wide-folder');
  return a;
}

/** A path with the home folder as `~`, for the eye. */
export function tilde(path: string, home: string | undefined): string {
  if (!home || home === '/') return path;
  if (path === home) return '~';
  return path.startsWith(home.endsWith('/') ? home : `${home}/`) ? `~/${path.slice(home.replace(/\/$/, '').length + 1)}` : path;
}

/** What the user typed as a folder, as an absolute path: `~` is the home folder, a relative path starts at `cwd`. */
export function expand(typed: string, cwd: string, home: string | undefined): string {
  const t = typed.trim();
  if (t === '') return '';
  if (home && (t === '~' || t.startsWith('~/'))) return resolve(home, t.slice(2));
  return isAbsolute(t) ? resolve(t) : resolve(cwd, t);
}

export interface FolderRow {
  path: string;
  /** "This folder" for the folder where paddock started, else the agent that ran there last. */
  note: string;
}

/** The folders to pick from: the current folder first, then the recent ones, each once. */
export function folderRows(cwd: string, recent: Recent[]): FolderRow[] {
  const rows: FolderRow[] = [{ path: cwd, note: t('launch.thisFolder') }];
  const seen = new Set([cwd]);
  for (const r of [...recent].sort((a, b) => (b.last ?? 0) - (a.last ?? 0))) {
    if (typeof r.project !== 'string' || r.project === '' || seen.has(r.project)) continue;
    seen.add(r.project);
    rows.push({ path: r.project, note: r.harness ? t('launch.lastAgent', { agent: oneLine(r.harness) }) : '' });
  }
  return rows;
}

/** The agent that ran last in a folder, when it is one of `names`: its index, else 0. */
export function lastAgent(folder: string, recent: Recent[], names: string[]): number {
  const hit = [...recent].sort((a, b) => (b.last ?? 0) - (a.last ?? 0)).find((r) => r.project === folder && r.harness !== undefined && names.includes(r.harness));
  return hit ? Math.max(0, names.indexOf(hit.harness as string)) : 0;
}

/** How the vendor says to install an agent, for the first start. Only where the package name is the vendor's own. */
export const INSTALL: Record<string, string> = {
  claude: 'npm install -g @anthropic-ai/claude-code',
  codex: 'npm install -g @openai/codex',
  gemini: 'npm install -g @google/gemini-cli',
  copilot: 'npm install -g @github/copilot',
  opencode: 'npm install -g opencode-ai',
};

/** Lines of at most `width` cells from words, at most `max` lines: the last one is cut. */
export function wrapWords(text: string, width: number, max: number): string[] {
  const out: string[] = [];
  if (width <= 0 || max <= 0) return out;
  let line = '';
  for (const word of text.split(/\s+/).filter((w) => w.length > 0).flatMap((w) => split(w, width))) {
    if (line === '') line = word;
    else if (cells(line) + 1 + cells(word) <= width) line += ` ${word}`;
    else {
      out.push(line);
      line = word;
    }
  }
  if (line) out.push(line);
  if (out.length <= max) return out;
  const kept = out.slice(0, max);
  kept[max - 1] = `${kept[max - 1]} ${out.slice(max).join(' ')}`;
  return kept;
}

// A word wider than the line, in pieces of at most `width` cells. Chinese
// and Japanese have no spaces between words, so a sentence is one "word".
function split(word: string, width: number): string[] {
  if (cells(word) <= width) return [word];
  const out: string[] = [];
  let piece = '';
  let used = 0;
  for (const ch of word) {
    const w = charCells(ch);
    if (used + w > width && piece !== '') {
      out.push(piece);
      piece = '';
      used = 0;
    }
    piece += ch;
    used += w;
  }
  if (piece !== '') out.push(piece);
  return out;
}
