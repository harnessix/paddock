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
  /** The id of the spawn that started this child (paddock-host spawn). */
  spawn_id?: string;
  /** The device instances that run for this sandbox now. */
  devices?: DeviceInstance[];
  /** The timers of the run. */
  timers?: TimerStatus[];
}

export interface DeviceInstance {
  device?: string;
  /** Where the instance runs: sandbox, host or shared. */
  run?: string;
  shape?: string;
  started?: number;
  running_seconds?: number;
}

export interface TimerStatus {
  name?: string;
  hard?: boolean;
  used?: number;
  budget?: number;
  left?: number;
  over?: boolean;
  stopped?: boolean;
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
  /** The device instances that several sandboxes share. */
  shared_devices?: DeviceInstance[];
  /** The sandboxed gateways of gateway agents (`paddock gateway NAME start`). */
  gateways?: Gateway[];
}

/** A sandboxed agent gateway, as `paddock ps --json` and `board --json` give it. */
export interface Gateway {
  name: string;
  state: 'starting' | 'running' | 'restarting' | 'failed' | 'stopped' | string;
  port?: number | null;
  url?: string | null;
  /** Seconds since the gateway that runs now started. */
  uptime?: number | null;
  started?: string | null;
  restarts?: number;
  last_exit?: string | null;
  clients?: number;
  pid?: number | null;
  backend?: string | null;
  network?: string | null;
  how?: string | null;
  unit?: string | null;
  reason?: string | null;
  /** A gateway on this port runs on the host without paddock. */
  host_gateway?: number | null;
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
  /** The folder of the loft's notes, when the project has a loft. */
  loft?: string;
  /** How many changes the notes of the loft have. paddock does not report it yet. */
  loft_commits?: number;
  tickets?: BoardTicket[];
  runs_without_ticket?: BoardRun[];
}

export interface Board {
  version?: number;
  approvals_waiting?: number;
  compile_slots?: { slot: number; holder: string }[];
  projects: BoardProject[];
  gateways?: Gateway[];
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
  /** The unread messages that are a question (`ask`). paddock does not report it yet. */
  unread_ask?: number;
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

/** `paddock device ls --json`: the tools of the user configuration. It is one list for every run. */
export interface DeviceDef {
  name: string;
  enabled?: boolean;
  shape?: string;
  /** Where the tool runs. `target` and `device` name the other computer of a remote tool. */
  run?: { run?: string; target?: string; device?: string };
  /** run: one instance per sandbox. shared: one for all. */
  scope?: string;
  /** ask: each call waits for you. allow: calls pass. */
  mode?: string;
  /** Who can reach the tool. None: every sandbox. */
  grant?: { grant?: string; to?: string }[] | null;
  cost?: number;
  budget?: number | null;
}

export interface DeviceList {
  mount?: boolean;
  builtin?: { name: string; enabled?: boolean }[];
  devices: DeviceDef[];
}

export interface KvMember {
  sandbox?: string;
  harness?: string;
  running?: boolean;
  frozen_since?: number | null;
}

export interface KvPasture {
  name: string;
  project?: string;
  /** A pasture of one run alone: not a shared yard. */
  solo?: boolean;
  members?: KvMember[];
  keys?: number;
  bytes?: number;
  leases?: number;
  ports?: { port?: number; sandbox?: string; harness?: string }[];
  folder?: string;
}

/** `paddock kv overview --json`. */
export interface KvOverview {
  pastures: KvPasture[];
  user?: { keys?: number; leases?: number };
  namespaces?: { name: string; keys?: number; leases?: number }[];
}

/** `paddock files --json`: what each sandbox changed. */
export interface FilesList {
  sandboxes: {
    id: string;
    running?: boolean;
    files?: { path?: string; size?: number; new?: boolean }[];
    project_changes?: { git?: boolean; commits?: string[]; changed?: string[]; changed_unknown?: boolean } | null;
  }[];
}

/** `paddock forage --json`: the use of the accounts of the agents. */
export interface Forage {
  windows: { agent?: string; label?: string; used?: number; resets?: number | null; overgrazing?: boolean; stale?: boolean }[];
}

/** `paddock horse list --json`: one saved session. */
export interface HorseSession {
  id: string;
  horse?: string;
  binding?: { harness?: string };
  created_ms?: number;
  origin_run?: string;
}
