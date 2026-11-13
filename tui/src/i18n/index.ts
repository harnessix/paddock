// The language of the interface. Every text on screen comes from the
// catalog: `en.ts` has the keys and the English text, and `locales/` has
// one file per language with the same keys. Commands, flags, paths, ids
// and the names of agents are not in the catalog: they stay as they are.

import { en, type Key } from './en.js';
import { de } from './locales/de.js';
import { es } from './locales/es.js';
import { fr } from './locales/fr.js';
import { it } from './locales/it.js';
import { ja } from './locales/ja.js';
import { ko } from './locales/ko.js';
import { ptBR } from './locales/pt-BR.js';
import { ru } from './locales/ru.js';
import { zhHans } from './locales/zh-Hans.js';
import { zhHant } from './locales/zh-Hant.js';

export type { Key };

import type { Entry } from './types.js';

export type { Entry, Plural } from './types.js';

/** A language file: every key of the English catalog. */
export type Catalog = { [K in Key]: Entry };

export const LANGS = ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko', 'es', 'fr', 'de', 'pt-BR', 'ru', 'it'] as const;
export type Lang = (typeof LANGS)[number];

export const CATALOGS: Record<Lang, Catalog> = {
  en,
  'zh-Hans': zhHans,
  'zh-Hant': zhHant,
  ja,
  ko,
  es,
  fr,
  de,
  'pt-BR': ptBR,
  ru,
  it,
};

/**
 * The language of a locale name such as `de_DE.UTF-8`, `zh_TW`, `zh-Hant-HK`,
 * `pt_BR` or `ja`. Undefined for a language that the catalog does not have.
 */
export function langOf(locale: string | undefined): Lang | undefined {
  if (!locale) return undefined;
  // Drop the encoding and the modifier: de_DE.UTF-8@euro.
  const name = locale.split(/[.@]/)[0]?.trim() ?? '';
  if (name === '' || name === 'C' || name === 'POSIX') return 'en';
  const parts = name.split(/[-_]/).map((p) => p.toLowerCase());
  const base = parts[0] ?? '';
  const rest = parts.slice(1);
  switch (base) {
    case 'en':
      return 'en';
    case 'zh': {
      if (rest.includes('hant')) return 'zh-Hant';
      if (rest.includes('hans')) return 'zh-Hans';
      if (rest.some((p) => p === 'tw' || p === 'hk' || p === 'mo')) return 'zh-Hant';
      return 'zh-Hans';
    }
    case 'pt':
      return 'pt-BR';
    case 'ja':
    case 'ko':
    case 'es':
    case 'fr':
    case 'de':
    case 'ru':
    case 'it':
      return base;
    default:
      return undefined;
  }
}

/**
 * The language from the environment: PADDOCK_LANG, then LC_ALL,
 * LC_MESSAGES and LANG. The first variable that is set decides, as in
 * POSIX. A language that the catalog does not have gives English.
 */
export function detectLang(env: NodeJS.ProcessEnv = process.env): Lang {
  for (const name of ['PADDOCK_LANG', 'LC_ALL', 'LC_MESSAGES', 'LANG']) {
    const v = env[name];
    if (v !== undefined && v !== '') return langOf(v) ?? 'en';
  }
  return 'en';
}

let current: Lang = detectLang();
let plurals = new Intl.PluralRules(current);

/** The language of the interface now. */
export function lang(): Lang {
  return current;
}

/** Change the language (the tests, and a start with another environment). */
export function setLang(l: Lang): void {
  current = l;
  plurals = new Intl.PluralRules(l);
}

export type Vars = Record<string, string | number>;

function pick(entry: Entry, n: number | undefined): string {
  if (typeof entry === 'string') return entry;
  if (n === undefined) return entry.other;
  const exact = n === 0 ? entry.zero : undefined;
  return exact ?? entry[plurals.select(n)] ?? entry.other;
}

/** Fill `{name}` placeholders. `{{` and `}}` are a literal brace. */
export function fill(text: string, vars?: Vars): string {
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (m, name: string) => (name in vars ? String(vars[name]) : m));
}

/**
 * The text of a key in the language of the interface, with its
 * placeholders filled. A plural entry picks its form by `vars.n`.
 */
export function t(key: Key, vars?: Vars): string {
  const cat = CATALOGS[current];
  const entry = cat[key] ?? en[key];
  const n = vars && typeof vars.n === 'number' ? vars.n : undefined;
  return fill(pick(entry, n), vars);
}

/** The placeholders of a text, sorted: `{n} of {total}` gives ['n', 'total']. */
export function placeholders(entry: Entry): string[] {
  const texts = typeof entry === 'string' ? [entry] : Object.values(entry).filter((v): v is string => typeof v === 'string');
  const names = new Set<string>();
  for (const text of texts) for (const m of text.matchAll(/\{(\w+)\}/g)) names.add(m[1] as string);
  return [...names].sort();
}
