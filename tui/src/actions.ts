// Every action, its key and its words. The command palette, the help
// overlay and the footer read this list, so no feature hides behind a key
// that is not written somewhere.

export type ActionId =
  | 'open'
  | 'back'
  | 'new'
  | 'stop'
  | 'split'
  | 'mark'
  | 'view'
  | 'write'
  | 'runs'
  | 'board'
  | 'approvals'
  | 'mail'
  | 'nextTab'
  | 'filter'
  | 'palette'
  | 'help'
  | 'mouse'
  | 'layout'
  | 'refresh'
  | 'quit';

export interface Action {
  id: ActionId;
  key: string;
  label: string;
  /** Changes something: hidden with --readonly. */
  changes: boolean;
}

export const ACTIONS: Action[] = [
  { id: 'open', key: '↵', label: 'open the selected item', changes: false },
  { id: 'back', key: 'q Esc', label: 'go back (quit on the first screen)', changes: false },
  { id: 'new', key: 'n', label: 'new run: harness, project, prompt file', changes: true },
  { id: 'stop', key: 's', label: 'stop the selected run (asks first)', changes: true },
  { id: 'split', key: 'v', label: 'split view of the marked runs, or view the selected run', changes: false },
  { id: 'mark', key: 'Space', label: 'mark a run for the split view', changes: false },
  { id: 'view', key: 'o', label: 'open the live view of the run in a new terminal window', changes: false },
  { id: 'write', key: 'w', label: 'write a message to the agent of the run', changes: true },
  { id: 'runs', key: 'r', label: 'show the runs', changes: false },
  { id: 'board', key: 'b', label: 'show the board', changes: false },
  { id: 'approvals', key: 'a', label: 'show the approvals that wait', changes: false },
  { id: 'mail', key: 'm', label: 'show the mail of the runs', changes: false },
  { id: 'nextTab', key: 'Tab', label: 'next tab or pane (Shift+Tab back)', changes: false },
  { id: 'filter', key: '/', label: 'filter the list', changes: false },
  { id: 'palette', key: ': Ctrl+P', label: 'command palette: every action', changes: false },
  { id: 'help', key: '?', label: 'this help', changes: false },
  { id: 'mouse', key: 'M', label: 'mouse on or off (off: the terminal selects text)', changes: false },
  { id: 'layout', key: 'L', label: 'runs as rooms (a card and a character per run) or as a list', changes: false },
  { id: 'refresh', key: 'R', label: 'read paddock again now', changes: false },
  { id: 'quit', key: 'Ctrl+C Q', label: 'quit', changes: false },
];

export function visibleActions(readonly: boolean): Action[] {
  return ACTIONS.filter((a) => !(readonly && a.changes));
}

export function searchActions(list: Action[], query: string): Action[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter((a) => a.label.toLowerCase().includes(q) || a.id.toLowerCase().includes(q) || a.key.toLowerCase() === q);
}
