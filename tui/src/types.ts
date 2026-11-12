// The JSON of paddock's commands, as far as the interface reads it. Every
// field is optional in practice: an older paddock can lack one.

export interface PsSandbox {
  id: string;
  short?: string;
  harness?: string;
  project?: string;
  backend?: string;
  network?: string;
  gui?: string;
  /** Who approves the agent's steps: auto, manual, or command (paddock wrap, apps). */
  approvals?: string;
  started?: number;
  parent?: string;
  pasture?: string | null;
  activity?: string | null;
  detached?: boolean;
  view?: boolean;
  view_tmux?: boolean;
  idle_since?: number;
  frozen_since?: number;
  frozen_reason?: string;
  stuck?: { since?: number } | null;
  model?: string;
  spec_task?: string;
  warnings?: string[];
  boundary_count?: number;
}

export interface PsEnded {
  id: string;
  short?: string;
  harness?: string;
  project?: string;
  ended?: number;
  status?: 'ok' | 'failed' | 'stopped' | 'idle' | 'unknown' | string;
  text?: string;
}

export interface Ps {
  api_version?: string;
  sandboxes: PsSandbox[];
  ended?: PsEnded[];
}

export interface BoardRun {
  id: string;
  harness?: string;
  model?: string;
  state?: string;
  last?: string;
  since?: string;
  card?: boolean;
  approvals?: string;
}

export interface BoardTicket {
  id: string;
  title?: string;
  file?: string;
  state?: string;
  runs?: BoardRun[];
}

export interface BoardProject {
  name: string;
  path?: string;
  tickets?: BoardTicket[];
  runs_without_ticket?: BoardRun[];
}

export interface Board {
  version?: number;
  approvals_waiting?: number;
  compile_slots?: { slot: number; holder: string }[];
  projects: BoardProject[];
}

export interface Connect {
  app?: string;
  host?: string;
  port?: number;
  count?: number;
}

export interface Pending {
  id: string;
  sandbox?: string;
  harness?: string;
  project?: string;
  capability?: string;
  summary?: string;
  detail?: string;
  reason?: string;
  created?: number;
  expires?: number;
  session_allowed?: boolean;
  started_by?: string[];
  choices?: string[];
  connect?: Connect;
  age_s?: number;
}

export interface Approvals {
  api_version?: string;
  pending: Pending[];
}

export interface MailBox {
  id: string;
  short?: string;
  state?: string;
  starter?: string | null;
  parent?: string | null;
  up?: number;
  unread_up?: number;
  unread_down?: number;
  last?: number | null;
}

export interface MailMessage {
  seq: number;
  ts?: number;
  from?: string;
  kind?: string;
  text?: string;
  dir?: string;
  event?: string;
  reply_to?: number;
  state?: string;
  refs?: string[];
}

export interface ViewTail {
  id?: string;
  source?: string | null;
  lines: string[];
}

export interface Harness {
  name: string;
  installed?: boolean;
  app?: boolean;
  description?: string;
}
