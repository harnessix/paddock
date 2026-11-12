// The only way the interface reads or changes anything: paddock's own
// commands. No logic of paddock is repeated here.

import { spawn } from 'node:child_process';
import { oneLine } from './clean.js';
import type {
  Approvals,
  Board,
  Harness,
  MailBox,
  MailMessage,
  Ps,
  ViewTail,
} from './types.js';

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

export function call(args: string[], opts: { stdin?: string; bin?: string } = {}): Promise<CallResult> {
  return new Promise((resolve) => {
    const child = spawn(opts.bin ?? paddockBin(), args, {
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
      resolve({ code: 127, stdout, stderr: `could not start ${opts.bin ?? paddockBin()}: ${e.message}` });
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
  return lines.find((l) => l.startsWith('error:')) ?? lines[lines.length - 1] ?? `paddock exited with code ${r.code}`;
}

async function checked(args: string[], stdin?: string): Promise<string> {
  const r = await call(args, { stdin });
  if (r.code !== 0 && isUnsupported(r)) throw new UnsupportedError(`needs a newer paddock: \`paddock ${args.join(' ')}\``);
  if (r.code !== 0) throw new PaddockError(errorLine(r));
  return r.stdout;
}

async function json<T>(args: string[]): Promise<T> {
  const out = await checked(args);
  try {
    return JSON.parse(out) as T;
  } catch {
    throw new PaddockError(`paddock ${args.join(' ')} printed no JSON`);
  }
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
  async sendMail(run: string, text: string): Promise<string> {
    return oneLine(await checked(['mail', 'send', run, '-'], text));
  },
};

/** The approvals mode of a new run; '' is the default of paddock's configuration. */
export type ApprovalsMode = '' | 'auto' | 'manual';

export type PaddockApi = typeof paddock;
