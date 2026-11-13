/** A text with plural forms, chosen by `Intl.PluralRules` on the variable `n`. */
export interface Plural {
  zero?: string;
  one?: string;
  two?: string;
  few?: string;
  many?: string;
  other: string;
}

export type Entry = string | Plural;
