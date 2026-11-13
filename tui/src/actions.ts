// Every action, its key and its words. The command palette, the help
// overlay and the footer read this list, so no feature hides behind a key
// that is not written somewhere.

import { t } from './i18n/index.js';
import { en } from './i18n/en.js';

export type ActionId =
  | 'home'
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
  | 'world'
  | 'plumbing'
  | 'inspector'
  | 'pause'
  | 'files'
  | 'applyAll'
  | 'backups'
  | 'pointer'
  | 'refresh'
  | 'quit';

export interface Action {
  id: ActionId;
  key: string;
  /** The words of the action, in the language of the interface. */
  readonly label: string;
  /** Changes something: hidden with --readonly. */
  changes: boolean;
}

function act(id: ActionId, key: string, changes: boolean): Action {
  return {
    id,
    key,
    get label() {
      return t(`action.${id}`);
    },
    changes,
  };
}

export const ACTIONS: Action[] = [
  act('home', 'H', false),
  act('open', '↵', false),
  act('back', 'q Esc', false),
  act('new', 'n', true),
  act('stop', 's', true),
  act('split', 'v', false),
  act('mark', 'Space', false),
  act('view', 'o', false),
  act('write', 'w', true),
  act('runs', 'r', false),
  act('board', 'b', false),
  act('approvals', 'a', false),
  act('mail', 'm', false),
  act('nextTab', 'Tab', false),
  act('filter', '/', false),
  act('palette', ': Ctrl+P', false),
  act('help', '?', false),
  act('mouse', 'M', false),
  act('layout', 'L', false),
  act('world', 'W', false),
  act('plumbing', 'P', false),
  act('inspector', 'I', false),
  act('pause', 'z', true),
  act('files', 'f', false),
  act('applyAll', '', true),
  act('backups', '', false),
  act('pointer', '', false),
  act('refresh', 'R', false),
  act('quit', 'Ctrl+C Q', false),
];

export function visibleActions(readonly: boolean): Action[] {
  return ACTIONS.filter((a) => !(readonly && a.changes));
}

export function searchActions(list: Action[], query: string): Action[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  // The English words find an action in every language.
  return list.filter(
    (a) => a.label.toLowerCase().includes(q) || en[`action.${a.id}`].toLowerCase().includes(q) || a.id.toLowerCase().includes(q) || a.key.toLowerCase() === q,
  );
}
