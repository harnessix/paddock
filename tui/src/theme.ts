// Semantic colour roles on the 16 ANSI colours. The terminal's palette
// decides the exact shade, so light and dark themes both work. No role
// carries meaning alone: every state also has a glyph and a word.

import { createContext, useContext } from 'react';

export type Role = 'accent' | 'success' | 'warning' | 'error' | 'muted' | 'normal';

export interface Theme {
  name: 'dark' | 'light';
  /** Box borders and text colours, or none at all in plain mode. */
  plain: boolean;
  color: Record<Exclude<Role, 'muted' | 'normal'>, string>;
}

const DARK: Theme['color'] = { accent: 'cyan', success: 'green', warning: 'yellow', error: 'red' };
// Yellow and cyan are faint on a light background in most palettes.
const LIGHT: Theme['color'] = { accent: 'blue', success: 'green', warning: 'magenta', error: 'red' };

export function makeTheme(name: 'dark' | 'light', plain = false): Theme {
  return { name, plain, color: name === 'light' ? LIGHT : DARK };
}

/**
 * The theme: --theme, then PADDOCK_TUI_THEME, then COLORFGBG ("fg;bg",
 * set by rxvt, Konsole and others: a background of 7 or 15 is light).
 */
export function detectTheme(flag: string | undefined, env: NodeJS.ProcessEnv): 'dark' | 'light' {
  const pick = flag ?? env.PADDOCK_TUI_THEME;
  if (pick === 'light' || pick === 'dark') return pick;
  const bg = env.COLORFGBG?.split(';').pop();
  if (bg === '7' || bg === '15') return 'light';
  return 'dark';
}

export const ThemeContext = createContext<Theme>(makeTheme('dark'));

export function useTheme(): Theme {
  return useContext(ThemeContext);
}

/** Props for Ink's Text for a role. */
export function roleProps(theme: Theme, role: Role): { color?: string; dimColor?: boolean } {
  if (role === 'normal') return {};
  if (role === 'muted') return { dimColor: true };
  return { color: theme.color[role] };
}
