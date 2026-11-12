// Semantic colour roles on the 16 ANSI colours. The terminal's palette
// decides the exact shade, so light and dark themes both work. No role
// carries meaning alone: every state also has a glyph and a word.

import { createContext, useContext } from 'react';

export type Role = 'accent' | 'success' | 'warning' | 'error' | 'muted' | 'normal';

export interface Theme {
  name: 'dark' | 'light';
  /** Box borders and text colours, or none at all in plain mode. */
  plain: boolean;
  /**
   * No colour codes at all (NO_COLOR): bold, dim, inverse and underline
   * carry the roles. The boxes stay.
   */
  mono: boolean;
  color: Record<Exclude<Role, 'muted' | 'normal'>, string>;
}

const DARK: Theme['color'] = { accent: 'cyan', success: 'green', warning: 'yellow', error: 'red' };
// Yellow and cyan are faint on a light background in most palettes.
const LIGHT: Theme['color'] = { accent: 'blue', success: 'green', warning: 'magenta', error: 'red' };

export function makeTheme(name: 'dark' | 'light', plain = false, mono = false): Theme {
  return { name, plain, mono, color: name === 'light' ? LIGHT : DARK };
}

/** NO_COLOR is set to any value but the empty one (no-color.org). */
export function noColor(env: NodeJS.ProcessEnv): boolean {
  return env.NO_COLOR !== undefined && env.NO_COLOR !== '';
}

/** The colour of the focus (borders, the selection mark), or none without colours. */
export function accentColor(theme: Theme): string | undefined {
  return theme.plain || theme.mono ? undefined : theme.color.accent;
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
export function roleProps(theme: Theme, role: Role): { color?: string; dimColor?: boolean; bold?: boolean } {
  if (role === 'normal') return {};
  if (role === 'muted') return { dimColor: true };
  // Without colours, what needs the eye is bold. Each role also has its glyph and its word.
  if (theme.mono) return role === 'success' ? {} : { bold: true };
  return { color: theme.color[role] };
}
