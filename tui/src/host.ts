// What the program around the interface gives it: whether it started on
// the home screen, and the handover of the terminal to a paddock command.
// A context, so the interface reads it where it needs it.

import { createContext, useContext } from 'react';
import type { Job } from './launch.js';

export interface Notice {
  text: string;
  role: 'success' | 'warning' | 'error' | 'muted';
}

export interface Host {
  /** The home screen is the first screen: Esc on a first screen goes back to it, and Esc on it quits. */
  home: boolean;
  /** Show the home screen now. Default: as `home`. True also after a command that the home screen started. */
  show?: boolean;
  /** What the command of the last handover did, to show once. */
  notice?: Notice;
  /**
   * Hand the terminal to `paddock ARGS` after the interface leaves. The
   * caller ends the interface next. Absent: this program cannot hand over.
   */
  run?: (job: Job) => void;
  /** Open the files of this run first: the interface came back from the editor. */
  resume?: { files: string };
}

export const HostContext = createContext<Host>({ home: false });

export function useHost(): Host {
  return useContext(HostContext);
}
