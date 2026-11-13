// The catalog: every language has every key of the English catalog, with
// the same placeholders and plural forms that its language needs. The
// language comes from PADDOCK_LANG, LC_ALL, LC_MESSAGES and LANG.
import { afterEach, describe, expect, test } from 'vitest';
import { CATALOGS, LANGS, detectLang, fill, langOf, lang, placeholders, setLang, t, type Entry, type Key } from '../src/i18n/index.js';
import { en } from '../src/i18n/en.js';
import { cells } from '../src/clean.js';

afterEach(() => setLang('en'));

const KEYS = Object.keys(en) as Key[];

describe('the catalog', () => {
  test('English has keys', () => {
    expect(KEYS.length).toBeGreaterThan(100);
  });

  describe.each(LANGS.filter((l) => l !== 'en'))('%s', (l) => {
    const cat = CATALOGS[l] as Record<string, Entry>;

    test('has every key of English, and no other key', () => {
      expect(Object.keys(cat).filter((k) => !(k in en))).toEqual([]);
      expect(KEYS.filter((k) => !(k in cat))).toEqual([]);
    });

    test('has the same placeholders as English in every key', () => {
      const wrong = KEYS.filter((k) => placeholders(cat[k] as Entry).join(',') !== placeholders(en[k]).join(','));
      expect(wrong.map((k) => `${k}: ${JSON.stringify(cat[k])}`)).toEqual([]);
    });

    test('a plural entry has the forms of its language, and an English plural stays a plural', () => {
      const rules = new Intl.PluralRules(l);
      const needed = rules.resolvedOptions().pluralCategories;
      const wrong: string[] = [];
      for (const k of KEYS) {
        const e = en[k] as Entry;
        const x = cat[k] as Entry;
        if (typeof e === 'string') {
          if (typeof x !== 'string') wrong.push(`${k}: a plural, but English has none`);
          continue;
        }
        if (typeof x === 'string') {
          // A language without plural forms (ja, ko, zh) may give one text.
          if (needed.length > 1) wrong.push(`${k}: no plural forms`);
          continue;
        }
        for (const c of needed) if (c !== 'other' && x[c] === undefined && !(c === 'many' && l !== 'ru')) wrong.push(`${k}: no "${c}" form`);
      }
      expect(wrong).toEqual([]);
    });

    test('is a translation: most of the texts differ from English', () => {
      const same = KEYS.filter((k) => JSON.stringify(cat[k]) === JSON.stringify(en[k]));
      expect(same.length / KEYS.length, same.slice(0, 40).join('\n')).toBeLessThan(0.35);
    });

    test('keeps the commands, the flags and the env variables of English as they are', () => {
      const wrong: string[] = [];
      const literal = /`[^`]+`|--[a-z][a-z-]+|\b[A-Z][A-Z0-9_]{3,}=?[A-Za-z0-9]*/g;
      for (const k of KEYS) {
        const text = (e: Entry) => (typeof e === 'string' ? e : e.other);
        for (const m of text(en[k]).matchAll(literal)) {
          if (!text(cat[k] as Entry).includes(m[0])) wrong.push(`${k}: ${m[0]} → ${text(cat[k] as Entry)}`);
        }
      }
      expect(wrong).toEqual([]);
    });
  });
});

describe('t', () => {
  test('fills the placeholders and picks the plural form of the language', () => {
    expect(fill('{n} of {total}', { n: 2, total: 5 })).toBe('2 of 5');
    expect(fill('{x} stays', {})).toBe('{x} stays');
    const plural = KEYS.find((k) => typeof en[k] !== 'string' && placeholders(en[k]).includes('n'));
    expect(plural).toBeDefined();
    const key = plural as Key;
    const vars = Object.fromEntries(placeholders(en[key]).map((p) => [p, p === 'n' ? 1 : p]));
    const one = t(key, vars);
    const many = t(key, { ...vars, n: 5 });
    expect(one).not.toBe(many);
    expect(one).toContain('1');
    expect(many).toContain('5');
    setLang('ru');
    // Russian: 1, 2-4, 5-20 take three forms.
    const ru = [1, 3, 5].map((n) => t(key, { ...vars, n }));
    expect(new Set(ru).size).toBe(3);
  });

  test('every text of every language fits in a terminal as cells: no control characters, no tabs', () => {
    for (const l of LANGS) {
      for (const [k, e] of Object.entries(CATALOGS[l])) {
        const texts = typeof e === 'string' ? [e] : Object.values(e);
        for (const s of texts) {
          expect(s, `${l} ${k}`).not.toMatch(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/);
          expect(cells(s), `${l} ${k}`).toBeGreaterThanOrEqual(0);
        }
      }
    }
  });
});

describe('the language of the environment', () => {
  test('PADDOCK_LANG first, then LC_ALL, LC_MESSAGES, LANG', () => {
    expect(detectLang({ PADDOCK_LANG: 'de', LC_ALL: 'ja_JP.UTF-8', LANG: 'fr_FR.UTF-8' })).toBe('de');
    expect(detectLang({ LC_ALL: 'ja_JP.UTF-8', LC_MESSAGES: 'ko_KR.UTF-8', LANG: 'fr_FR.UTF-8' })).toBe('ja');
    expect(detectLang({ LC_MESSAGES: 'ko_KR.UTF-8', LANG: 'fr_FR.UTF-8' })).toBe('ko');
    expect(detectLang({ LANG: 'fr_FR.UTF-8' })).toBe('fr');
    expect(detectLang({ LC_ALL: '', LANG: 'es_ES.UTF-8' })).toBe('es');
    expect(detectLang({})).toBe('en');
  });

  test('an unknown language is English, and so are C and POSIX', () => {
    expect(detectLang({ LANG: 'nl_NL.UTF-8' })).toBe('en');
    expect(detectLang({ LC_ALL: 'C', LANG: 'de_DE.UTF-8' })).toBe('en');
    expect(detectLang({ LC_ALL: 'C.UTF-8' })).toBe('en');
    expect(detectLang({ LANG: 'POSIX' })).toBe('en');
    expect(detectLang({ PADDOCK_LANG: 'xx' })).toBe('en');
  });

  test('the names of the locales map to the languages of the catalog', () => {
    const cases: [string, string][] = [
      ['zh_CN.UTF-8', 'zh-Hans'],
      ['zh_SG', 'zh-Hans'],
      ['zh-Hans', 'zh-Hans'],
      ['zh_TW.UTF-8', 'zh-Hant'],
      ['zh_HK', 'zh-Hant'],
      ['zh-Hant-HK', 'zh-Hant'],
      ['zh', 'zh-Hans'],
      ['ja_JP.eucJP', 'ja'],
      ['ko_KR', 'ko'],
      ['pt_BR.UTF-8', 'pt-BR'],
      ['pt_PT', 'pt-BR'],
      ['de_AT.UTF-8@euro', 'de'],
      ['ru_RU.KOI8-R', 'ru'],
      ['it_IT', 'it'],
      ['es_MX', 'es'],
      ['fr_CA', 'fr'],
      ['en_GB.UTF-8', 'en'],
    ];
    for (const [locale, want] of cases) expect(langOf(locale), locale).toBe(want);
  });

  test('the tests run in English', () => {
    expect(lang()).toBe('en');
  });
});
