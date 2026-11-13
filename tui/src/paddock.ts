// The only way the interface reads or changes anything: paddock's own
// commands. No logic of paddock is repeated here.

import { spawn } from 'node:child_process';
import { oneLine } from './clean.js';
import { t } from './i18n/index.js';
import type {
  Approvals,
  Board,
  DeviceList,
  FilesList,
  Forage,
  Harness,
  HorseSession,
  KvOverview,
  MailBox,
  MailMessage,
  Ps,
  ViewTail,
} from './types.js';
import type { Explain, Recent } from './launch.js';
import { normalApply, normalChanges, normalRestore, type ApplyResult, type BackupsDoc, type ChangesDoc, type CopiesDoc, type OnConflict, type PathsResult, type PointerDoc, type RestoreResult } from './copy.js';

export class PaddockError extends Error {}

/** This paddock does not know the command: it is older than the interface. */
export class UnsupportedError extends PaddockError {}

/** Whether clap refused the command line: a subcommand or an option that this paddock lacks. */
export function isUnsupported(r: CallResult): boolean {
  return r.code === 2 && /unrecognized subcommand|unexpected argument/.test(r.stderr);
}

export interface CallResult {
  code: number;
  stdout: string;
  stderr: string;
}

const TIMEOUT_MS = 20_000;
const MAX_OUTPUT = 32 << 20;

/** The paddock binary: PADDOCK_BIN (set by `paddock tui`), else paddock on PATH. */
export function paddockBin(env: NodeJS.ProcessEnv = process.env): string {
  return env.PADDOCK_BIN && env.PADDOCK_BIN.length > 0 ? env.PADDOCK_BIN : 'paddock';
}

export function call(args: string[], opts: { stdin?: string; bin?: string; cwd?: string } = {}): Promise<CallResult> {
  return new Promise((resolve) => {
    const child = spawn(opts.bin ?? paddockBin(), args, {
      cwd: opts.cwd,
      stdio: [opts.stdin === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'],
      env: { ...process.env, NO_COLOR: '1', COLUMNS: '200' },
    });
    let stdout = '';
    let stderr = '';
    let size = 0;
    const timer = setTimeout(() => child.kill('SIGTERM'), TIMEOUT_MS);
    child.stdout?.setEncoding('utf8');
    child.stderr?.setEncoding('utf8');
    child.stdout?.on('data', (d: string) => {
      size += d.length;
      if (size <= MAX_OUTPUT) stdout += d;
    });
    child.stderr?.on('data', (d: string) => {
      if (stderr.length < 64 << 10) stderr += d;
    });
    if (opts.stdin !== undefined && child.stdin) {
      child.stdin.on('error', () => undefined);
      child.stdin.end(opts.stdin);
    }
    child.on('error', (e) => {
      clearTimeout(timer);
      resolve({ code: 127, stdout, stderr: t('paddock.couldNotStart', { bin: opts.bin ?? paddockBin(), error: e.message }) });
    });
    child.on('close', (code, signal) => {
      clearTimeout(timer);
      resolve({ code: code ?? (signal ? 128 : 1), stdout, stderr });
    });
  });
}

/** The last line of an error, one line, without escapes. */
export function errorLine(r: CallResult): string {
  const lines = r.stderr
    .split('\n')
    .map((l) => oneLine(l))
    .filter((l) => l.length > 0);
  // clap ends with a usage hint: the "error:" line says what went wrong.
  return lines.find((l) => l.startsWith('error:')) ?? lines[lines.length - 1] ?? t('paddock.exitCode', { code: r.code });
}

async function checked(args: string[], stdin?: string): Promise<string> {
  const r = await call(args, { stdin });
  if (r.code !== 0 && isUnsupported(r)) throw new UnsupportedError(t('paddock.needsNewer', { command: `paddock ${args.join(' ')}` }));
  if (r.code !== 0) throw new PaddockError(errorLine(r));
  return r.stdout;
}

async function json<T>(args: string[]): Promise<T> {
  const out = await checked(args);
  try {
    return JSON.parse(out) as T;
  } catch {
    throw new PaddockError(t('paddock.noJson', { command: `paddock ${args.join(' ')}` }));
  }
}

/**
 * The JSON result of an action. An apply that left a conflict exits with
 * an error after its JSON: the JSON is the answer then too.
 */
async function result<T>(args: string[], cwd?: string): Promise<T> {
  const r = await call(args, { cwd });
  try {
    const v: unknown = JSON.parse(r.stdout);
    if (v && typeof v === 'object' && 'api_version' in v) return v as T;
  } catch {
    // No JSON: the error line says why.
  }
  if (r.code !== 0 && isUnsupported(r)) throw new UnsupportedError(t('paddock.needsNewer', { command: `paddock ${args.slice(0, 2).join(' ')}` }));
  if (r.code !== 0) throw new PaddockError(errorLine(r));
  throw new PaddockError(t('paddock.noJson', { command: `paddock ${args[0]}` }));
}

export const paddock = {
  async ps(): Promise<Ps> {
    const v = await json<Partial<Ps>>(['ps', '--json']);
    return { ...v, sandboxes: v.sandboxes ?? [], ended: v.ended ?? [] };
  },
  async board(): Promise<Board> {
    const v = await json<Partial<Board>>(['board', '--json']);
    return { ...v, projects: v.projects ?? [] };
  },
  async approvals(): Promise<Approvals> {
    const v = await json<Partial<Approvals>>(['approvals', '--json']);
    return { ...v, pending: v.pending ?? [] };
  },
  async mailBoxes(): Promise<MailBox[]> {
    return (await json<MailBox[]>(['mail', 'ls', '--json'])) ?? [];
  },
  async mailMessages(run: string): Promise<MailMessage[]> {
    return (await json<MailMessage[]>(['mail', 'ls', run, '--json'])) ?? [];
  },
  async mailShow(run: string, seq: number): Promise<MailMessage> {
    return json<MailMessage>(['mail', 'show', run, String(seq), '--json']);
  },
  async tail(run: string, lines: number): Promise<ViewTail> {
    const v = await json<Partial<ViewTail>>(['view', run, '--tail', String(lines), '--json']);
    return { ...v, lines: v.lines ?? [] };
  },
  async harnesses(): Promise<Harness[]> {
    const v = await json<{ harnesses?: Harness[] }>(['list', '--json']);
    return v.harnesses ?? [];
  },
  /** The projects where runs started, newest first. */
  async recent(): Promise<Recent[]> {
    const v = await json<{ projects?: Recent[] }>(['recent', '--json']);
    return Array.isArray(v.projects) ? v.projects : [];
  },
  /**
   * What paddock would start for an agent in a folder, and what the folder
   * guard says. A refused folder is an answer, not a failure: paddock
   * prints `folder_check` and exits with an error, and both come back.
   */
  async explain(harness: string, folder: string): Promise<Explain> {
    const r = await call(['explain', harness, folder, '--json']);
    let v: Explain = {};
    try {
      const parsed: unknown = JSON.parse(r.stdout);
      if (parsed && typeof parsed === 'object') v = parsed as Explain;
    } catch {
      // No JSON: the error line says why.
    }
    const out: Explain = { network: v.network, approvals: v.approvals, folder_check: v.folder_check };
    if (r.code !== 0) out.error = isUnsupported(r) ? t('paddock.noExplain') : errorLine(r);
    return out;
  },
  /** `paddock approvals answer ID ...`. A text goes through stdin, never the process list. */
  async answer(id: string, args: string[], text?: string): Promise<string> {
    const full = ['approvals', 'answer', id, ...args];
    if (text !== undefined) full.push('--text', '-');
    return oneLine(await checked(full, text));
  },
  async stop(id: string): Promise<string> {
    return oneLine(await checked(['stop', id]));
  },
  async view(id: string, write = false): Promise<string> {
    return oneLine(await checked(write ? ['view', id, '--write'] : ['view', id]));
  },
  /** Whether this paddock has `paddock split` (SPLIT1). */
  async hasSplit(): Promise<boolean> {
    const r = await call(['split', '--help']);
    return r.code === 0;
  },
  async split(ids: string[]): Promise<string> {
    return oneLine(await checked(['split', ...ids]));
  },
  /** `approvals`: '' leaves the mode to paddock's configuration. */
  async run(harness: string, project: string, promptFile: string, approvals: ApprovalsMode = ''): Promise<string> {
    const mode = approvals === 'manual' ? ['--manual'] : approvals === 'auto' ? ['--auto'] : [];
    return oneLine(await checked(['run', harness, project, '--detach', '--prompt-file', promptFile, ...mode]));
  },
  /** The tools of the user configuration: one list for every run. */
  async devices(): Promise<DeviceList> {
    const v = await json<Partial<DeviceList>>(['device', 'ls', '--json']);
    return { ...v, devices: v.devices ?? [] };
  },
  /** The shared notes: every pasture with its members, the user scope and the namespaces. */
  async kvOverview(): Promise<KvOverview> {
    const v = await json<Partial<KvOverview>>(['kv', 'overview', '--json']);
    return { ...v, pastures: v.pastures ?? [] };
  },
  /** What each sandbox changed in its home and in its project. */
  async files(): Promise<FilesList> {
    const v = await json<Partial<FilesList>>(['files', '--json']);
    return { ...v, sandboxes: v.sandboxes ?? [] };
  },
  /** The use of the accounts of the agents. */
  async forage(): Promise<Forage> {
    const v = await json<Partial<Forage>>(['forage', '--json']);
    return { ...v, windows: v.windows ?? [] };
  },
  /** The saved sessions of horse mode. An empty list: the feature is off or has none. */
  async horses(): Promise<HorseSession[]> {
    return (await json<HorseSession[]>(['horse', 'list', '--json'])) ?? [];
  },
  /** `paddock freeze ID [--tree]`: the run stops in place and keeps its memory. */
  async freeze(id: string, tree = false): Promise<string> {
    return oneLine(await checked(['freeze', id, ...(tree ? ['--tree'] : [])]));
  },
  /** `paddock thaw ID [--tree]`. */
  async thaw(id: string, tree = false): Promise<string> {
    return oneLine(await checked(['thaw', id, ...(tree ? ['--tree'] : [])]));
  },
  async sendMail(run: string, text: string): Promise<string> {
    return oneLine(await checked(['mail', 'send', run, '-'], text));
  },

  // ---- the copy of a run (COPY1) ----

  /** Every run that works on a copy, with the counts of its changes. */
  async copies(): Promise<CopiesDoc> {
    const v = await json<Partial<CopiesDoc>>(['changes', '--json']);
    return { ...v, runs: Array.isArray(v.runs) ? v.runs : [] };
  },
  /** What a run changed in its copy. */
  async changes(run: string): Promise<ChangesDoc> {
    return normalChanges(await json<Partial<ChangesDoc>>(['changes', run, '--json']));
  },
  /** The diff of one file, as the unified text that paddock makes. */
  async diff(run: string, path: string): Promise<string> {
    return checked(['changes', run, `--diff=${path}`]);
  },
  /** Write changes to the folder. `token`: only when the list is still the one that the user saw. */
  async apply(run: string, paths: string[], on: OnConflict, token: string): Promise<ApplyResult> {
    return normalApply(await result<Partial<ApplyResult>>(['apply', run, '--on-conflict', on, ...(token ? ['--token', token] : []), '--yes', '--json', '--', ...paths]));
  },
  async keep(run: string, paths: string[], off: boolean): Promise<PathsResult> {
    const v = await result<Partial<PathsResult>>(['keep', run, '--json', ...(off ? ['--off'] : []), '--', ...paths]);
    return { ...v, paths: Array.isArray(v.paths) ? v.paths : [] };
  },
  /** Drop changes from the copy. Never without a path: that would remove the whole copy. */
  async discardChanges(run: string, paths: string[]): Promise<PathsResult> {
    if (paths.length === 0) throw new PaddockError(t('fx.paddock.noDiscard'));
    const v = await result<Partial<PathsResult>>(['discard', run, '--yes', '--json', '--', ...paths]);
    return { ...v, paths: Array.isArray(v.paths) ? v.paths : [] };
  },
  /** Take the last apply or restore of a project back. `paddock undo` means the folder that it runs in. */
  async undoApply(project: string): Promise<RestoreResult> {
    return normalRestore(await result<Partial<RestoreResult>>(['undo', '--yes', '--json'], project));
  },
  async backups(project: string): Promise<BackupsDoc> {
    const v = await json<Partial<BackupsDoc>>(['backups', project, '--json']);
    return { ...v, backups: Array.isArray(v.backups) ? v.backups : [], snapshots: Array.isArray(v.snapshots) ? v.snapshots : [] };
  },
  async restore(backup: string, project: string): Promise<RestoreResult> {
    return normalRestore(await result<Partial<RestoreResult>>(['restore', backup, '--project', project, '--yes', '--json']));
  },
  async pointer(project: string): Promise<PointerDoc> {
    const v = await json<Partial<PointerDoc>>(['pointer', 'status', project, '--json']);
    return { ...v, state: typeof v.state === 'string' ? v.state : null };
  },
  /** `paddock pointer new|create|link PROJECT`: the line that paddock prints. */
  async pointerDo(verb: 'new' | 'create' | 'link', project: string): Promise<string> {
    return oneLine(await checked(['pointer', verb, project, ...(verb === 'create' ? [] : ['--yes'])]));
  },
};

/** The approvals mode of a new run; '' is the default of paddock's configuration. */
export type ApprovalsMode = '' | 'auto' | 'manual';

export type PaddockApi = typeof paddock;
