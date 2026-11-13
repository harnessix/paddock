// Reduce Motion: whether the world shows each change without motion.
// The first of these that says so decides:
//   1. --reduce-motion on the command line.
//   2. PADDOCK_TUI_MOTION: reduce (or off, 0, none) turns motion off,
//      full (or on, 1) keeps it on whatever the desktop says.
//   3. The setting of the desktop, where a command reads it: macOS
//      (Reduce motion), GNOME (animations off) and KDE (animation speed
//      zero). A terminal has no report of its own for this setting, so
//      the desktop of the machine that the terminal runs on is the
//      nearest fact. Over SSH that machine is not this one: no command
//      runs.
// NO_COLOR and --plain are not in this list: they turn the art off, and
// without art nothing moves at all.

import { spawnSync } from 'node:child_process';

/** PADDOCK_TUI_MOTION=reduce (or off, 0): the world shows each change at once. */
export function reducedMotion(env: NodeJS.ProcessEnv): boolean {
  const v = env.PADDOCK_TUI_MOTION?.toLowerCase();
  return v === 'reduce' || v === 'reduced' || v === 'off' || v === '0' || v === 'none';
}

/** PADDOCK_TUI_MOTION=full (or on, 1): motion, whatever the desktop says. */
export function fullMotion(env: NodeJS.ProcessEnv): boolean {
  const v = env.PADDOCK_TUI_MOTION?.toLowerCase();
  return v === 'full' || v === 'on' || v === '1' || v === 'all';
}

/** Run a command and give its output, or null when it failed or took too long. */
export type ReadCommand = (cmd: string, args: string[]) => string | null;

const read: ReadCommand = (cmd, args) => {
  try {
    const r = spawnSync(cmd, args, { encoding: 'utf8', timeout: 300, stdio: ['ignore', 'pipe', 'ignore'] });
    return r.status === 0 && typeof r.stdout === 'string' ? r.stdout : null;
  } catch {
    return null;
  }
};

/**
 * The Reduce Motion setting of the desktop: true, false, or null when no
 * command of this machine reads it.
 */
export function desktopReducedMotion(env: NodeJS.ProcessEnv, platform: NodeJS.Platform, run: ReadCommand = read): boolean | null {
  // The terminal is on another machine: its desktop is not this one.
  if (env.SSH_CONNECTION || env.SSH_TTY) return null;
  if (platform === 'darwin') {
    const v = run('defaults', ['read', 'com.apple.universalaccess', 'reduceMotion']);
    return v === null ? null : v.trim() === '1';
  }
  if (platform !== 'linux') return null;
  const desktop = (env.XDG_CURRENT_DESKTOP ?? '').toLowerCase();
  if (/gnome|unity|cinnamon|budgie|pantheon/.test(desktop)) {
    const v = run('gsettings', ['get', 'org.gnome.desktop.interface', 'enable-animations']);
    return v === null ? null : v.trim() === 'false';
  }
  if (desktop.includes('kde')) {
    const v = run('kreadconfig6', ['--file', 'kdeglobals', '--group', 'KDE', '--key', 'AnimationDurationFactor']) ?? run('kreadconfig5', ['--file', 'kdeglobals', '--group', 'KDE', '--key', 'AnimationDurationFactor']);
    if (v === null || v.trim() === '') return null;
    return Number(v.trim()) === 0;
  }
  return null;
}

/** Whether the world reduces its motion: the flag, then the variable, then the desktop. */
export function wantsReducedMotion(flag: boolean, env: NodeJS.ProcessEnv, platform: NodeJS.Platform = process.platform, run: ReadCommand = read): boolean {
  if (flag || reducedMotion(env)) return true;
  if (fullMotion(env)) return false;
  return desktopReducedMotion(env, platform, run) === true;
}
