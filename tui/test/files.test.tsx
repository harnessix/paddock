// The file manager at work: the keys and the mouse of the design
// (.paddock/design/COPY1-tui-files.md, sections 9 and 10), every action
// against a fake paddock, the second confirm of the ⚠ choices, the
// conflict that asks before an apply, and read-only mode.
import { existsSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, describe, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { ArtLayer } from '../src/art-layer.js';
import { actions, click, fakeWorld, renderApp, sleep, until, writeJson, type Call, type Rendered } from './helpers.js';
import { demoCopy, RUN } from './files-demo.js';
import type { ChangesDoc } from '../src/copy.js';

const HOME = process.env.HOME;
afterEach(() => {
  cleanup();
  process.env.HOME = HOME;
});

const strip = (s: string): string => s.replace(/\u001b\[[\d;]*m/g, '');
const text = (r: Rendered): string => strip(r.frame());
const see = (r: Rendered, words: string): Promise<string> => r.waitFor((f) => strip(f).includes(words));
const gone = (r: Rendered, words: string): Promise<string> => r.waitFor((f) => !strip(f).includes(words));
/** The row of the tree or of a list that holds a text. */
const rowOf = (r: Rendered, words: string): string => text(r).split('\n').find((l) => l.startsWith('│') && l.includes(words)) ?? '';
/** The row of the tree with a name: the tree is the first 32 cells of the screen. */
const isTreeRow = (l: string): boolean => l.startsWith('│') && l.slice(0, 32).endsWith('│') && !l.includes('▏');
const treeRow = (r: Rendered, name: string): string =>
  text(r)
    .split('\n')
    .filter(isTreeRow)
    .map((l) => l.slice(0, 32))
    .find((l) => l.includes(name)) ?? '';
/** Whether the tree has a row with this name. */
const inTree = (name: string) => (f: string): boolean => strip(f).split('\n').some((l) => isTreeRow(l) && l.slice(0, 32).includes(name));
/** The second confirm counts only after 300 ms: a key repeat is none. */
const pause = (): Promise<unknown> => sleep(380);
const wheel = (x: number, y: number, down: boolean): string => `\u001b[<${down ? 65 : 64};${x + 1};${y + 1}M`;

/** Where a text stands on the screen, in cells. */
function at(r: Rendered, words: string): { x: number; y: number } {
  const lines = text(r).split('\n');
  for (let y = 0; y < lines.length; y++) {
    const i = (lines[y] as string).indexOf(words);
    if (i >= 0) return { x: Array.from((lines[y] as string).slice(0, i)).length, y };
  }
  throw new Error(`"${words}" is not on the screen:\n${lines.join('\n')}`);
}

interface Opened {
  r: Rendered;
  dir: string;
  project: string;
}

/** The files of the run of the test. The first file, docs/README.md, is selected. */
async function open(o: Parameters<typeof renderApp>[0] = {}, edit?: (doc: ChangesDoc) => void, world?: (dir: string) => void): Promise<Opened> {
  let project = '';
  const dir = fakeWorld((d) => {
    project = demoCopy(d, edit).project;
    world?.(d);
  });
  const r = await renderApp({ columns: 80, rows: 24, ...o });
  await see(r, '7c1e42');
  await r.press('f');
  await see(r, '[files]');
  await r.waitFor((f) => !strip(f).includes('Reading paddock changes'));
  return { r, dir, project };
}

/** Move the selection to the row of a file or a folder. */
async function goTo(r: Rendered, name: string): Promise<void> {
  await r.press('g');
  for (let i = 0; i < 40 && !treeRow(r, name).startsWith('│ ▸'); i++) await r.press('j');
  expect(treeRow(r, name)).toMatch(/^│ ▸/);
}

const did = (dir: string): string[] => actions(dir).map((c) => c.args.join(' '));
const last = (dir: string): Call => actions(dir).at(-1) as Call;
const read = (p: string): string => readFileSync(p, 'utf8');

describe('where the user enters', () => {
  test('f on a run of the list opens its files, and Esc goes back', async () => {
    const { r } = await open();
    expect(text(r)).toContain('claude · webshop · running');
    expect(text(r)).toContain('copy: 4 changed, 2 new, 1 deleted');
    expect(text(r)).toContain('[files]');
    expect(text(r)).toContain('pointer: ● linked');
    await r.press('\u001b');
    await see(r, 'Runs · 2 live');
  });

  test('on the run screen, f and Tab open the tab, and the screen names the copy', async () => {
    const { r } = await open();
    await r.press('\u001b');
    await see(r, 'Runs · 2 live');
    await r.press('\r');
    await see(r, '7 changes · 1 conflict: press f');
    expect(text(r)).toMatch(/f files/);
    await r.press('\t');
    await see(r, 'Files · changes');
    // The tabs of the run: a click on "run" goes back to it.
    const tab = at(r, ' run ');
    await r.press(click(tab.x + 2, tab.y));
    await see(r, 'Run · claude-7c1e42');
    await r.press('f');
    await see(r, 'Files · changes');
  });

  test('the command palette lists the files, the apply of all, the backups and the pointer', async () => {
    const { r } = await open();
    await r.press('\u001b');
    await see(r, 'Runs · 2 live');
    await r.press(':');
    await r.press('this run');
    const f = await see(r, 'Files of this run');
    expect(f).toContain('Apply all changes of this run');
    expect(f).toContain('Backups of the project of this run');
    await r.press('\u001b');
    await r.press(':');
    await r.press('Pointer state');
    await see(r, 'Pointer state');
    await r.press('\r');
    await see(r, 'encrypted (LUKS)');
  });

  test('a room card has the line of its copy, and a click on it opens the files', async () => {
    const dir = fakeWorld((d) => void demoCopy(d));
    void dir;
    const r = await renderApp({ columns: 120, rows: 34, art: new ArtLayer({ mode: 'truecolor', sink: () => undefined, screen: () => ({ columns: 120, rows: 34 }) }) });
    await see(r, '◆ 7 changes · 1 conflict');
    const line = at(r, '◆ 7 changes');
    await r.press(click(line.x + 3, line.y));
    await see(r, 'Files · changes');
  });

  test('the help has the keys of the files', async () => {
    const { r } = await open();
    await r.press('?');
    await see(r, 'Help · every key');
    const seen = new Set<string>();
    for (let i = 0; i < 60; i++) {
      for (const l of text(r).split('\n')) seen.add(l);
      await r.press('j');
    }
    const all = [...seen].join('\n');
    expect(all).toContain('Files of this run');
    expect(all).toContain('A apply, K keep in the copy');
    expect(all).toContain('n N the next and the previous change');
  });
});

describe('the keys of the tree', () => {
  test('j k g G move, the row has ▸ and the buttons, and the position counts the files', async () => {
    const { r } = await open();
    await see(r, '1 of 7 · 1 conflict');
    expect(treeRow(r, 'README.md')).toMatch(/▸ +! README\.md +\[A\]\[K\]\[D\]/);
    await r.press('j');
    await see(r, 'src/ · 5 files');
    expect(treeRow(r, 'src/ ')).toMatch(/▸▾ src\/ +\[A\]\[K\]\[D\]/);
    await r.press('G');
    await see(r, '7 of 7');
    expect(treeRow(r, 'logo.png')).toContain('▸');
    await r.press('k');
    await see(r, '6 of 7');
    await r.press('g');
    // The first row is the project root: it stands for all.
    await see(r, 'webshop/ · 7 files');
    expect(text(r)).toContain('A K D act on all 7');
  });

  test('← closes a folder and → opens it, Enter does both, and a closed folder keeps its counts', async () => {
    const { r } = await open();
    await goTo(r, 'src/ ');
    await r.press('\u001b[D');
    await r.waitFor((f) => !inTree('cart.ts')(f));
    expect(treeRow(r, 'src/ ')).toMatch(/▸▸ src\//);
    expect(text(r)).toContain('src/ · 5 files · ~2 +2 =1');
    await r.press('\u001b[C');
    await r.waitFor(inTree('cart.ts'));
    await r.press('\r');
    await r.waitFor((f) => !inTree('cart.ts')(f));
    await r.press('\r');
    await r.waitFor(inTree('cart.ts'));
    // ← on a file goes to its folder.
    await goTo(r, 'cart.ts');
    await r.press('\u001b[D');
    await see(r, 'src/ · 5 files');
  });

  test('c changes the filter, and the pane title names it', async () => {
    const { r } = await open();
    await r.press('c');
    await see(r, 'Files · ready to apply');
    expect(text(r)).not.toContain('README.md');
    expect(text(r)).not.toContain('old.ts');
    expect(text(r)).toContain('cart.ts');
    await r.press('c');
    await see(r, 'Files · conflicts');
    expect(text(r)).toContain('README.md');
    expect(text(r)).not.toContain('cart.ts');
    await r.press('c');
    await see(r, 'Files · changes');
  });

  test('/ filters by name, Enter keeps the name and Esc clears it', async () => {
    const { r } = await open();
    await r.press('/');
    await r.press('tax');
    await see(r, '/ tax');
    await gone(r, 'cart.ts');
    expect(text(r)).toContain('tax.ts');
    // What is typed is text: the letter a is no command.
    expect(text(r)).not.toContain('Approvals · nothing waits');
    await r.press('\r');
    await see(r, 'Files · changes · "tax"');
    await r.press('\u001b');
    await see(r, 'cart.ts');
    await r.press('/');
    await r.press('zzz');
    await see(r, 'No file matches');
  });

  test('the lower case keys of a run keep their meaning: a approvals, m mail', async () => {
    const { r } = await open();
    await r.press('m');
    await see(r, `Mail · ${RUN}`);
    await r.press('\u001b');
    await see(r, 'Files · changes');
    await r.press('a');
    await see(r, 'Approvals · nothing waits');
  });
});

describe('the diff', () => {
  test('Enter gives the diff the whole width, n and N go through the changes, Esc goes back', async () => {
    const { r } = await open();
    await goTo(r, 'cart.ts');
    await see(r, '@@ line 38');
    await r.press('\r');
    await see(r, 'src/cart.ts · changed · +3 -2 · original ↔ copy');
    expect(text(r)).not.toContain('Files · changes');
    expect(text(r)).toContain('change 1 of 2 · unified');
    expect(text(r)).toMatch(/-42 {5}\(s, i\) => s \+ i\.price, 0\);/);
    expect(text(r)).toMatch(/\+42 {5}\(s, i\) => s \+ i\.price \* i\.qty,/);
    await r.press('n');
    await see(r, 'change 2 of 2');
    await r.press('n');
    await see(r, 'This is the last change');
    await r.press('N');
    await see(r, 'change 1 of 2');
    await r.press('\u001b');
    await see(r, 'Files · changes');
  });

  test('a long line is cut with …, and ← → move the view sideways', async () => {
    const { r } = await open();
    await goTo(r, 'cart.ts');
    await r.press('\r');
    await see(r, 'original ↔ copy');
    await see(r, 'item.count');
    expect(rowOf(r, 'item.count')).toMatch(/…/);
    expect(text(r)).not.toContain('for each piece of it');
    for (let i = 0; i < 5; i++) await r.press('\u001b[C');
    await see(r, 'from column 41');
    expect(text(r)).toContain('for each piece of it');
    // The line numbers and the signs stay in place.
    expect(rowOf(r, 'for each piece of it')).toMatch(/^│ [-+]\d+ …/);
    await r.press('\u001b[D');
    await see(r, 'from column 33');
  });

  test('t needs 120 columns: below that the toast says so', async () => {
    const { r } = await open();
    await goTo(r, 'cart.ts');
    await r.press('t');
    await see(r, 'Side by side needs 120 columns');
  });

  test('from 120 columns the diff is side by side with ~ on both sides, and t changes to unified', async () => {
    const { r } = await open({ columns: 120, rows: 34 });
    await goTo(r, 'cart.ts');
    await see(r, 'original │ copy');
    expect(rowOf(r, 's + i.price, 0)')).toMatch(/~42 .*│~42 /);
    expect(rowOf(r, '+43 ')).toMatch(/│\+43 {5}0\);/);
    await r.press('t');
    await see(r, 'change 1 of 2 · unified');
    expect(text(r)).toMatch(/-42 {5}\(s, i\)/);
  });

  test('a new file shows all lines with +, and a file that is no text shows both sizes', async () => {
    const { r } = await open();
    await goTo(r, 'tax.ts');
    await see(r, '+1 export const VAT = 0.19;');
    expect(text(r)).toContain('src/tax.ts · new · 40 lines');
    await goTo(r, 'logo.png');
    await see(r, 'An image.');
    expect(text(r)).toMatch(/original {2}47 KB/);
    expect(text(r)).toMatch(/copy {6}no file/);
  });

  test('a file above 1 MB is not shown: e opens it in the editor', async () => {
    const { r } = await open({}, (doc) => {
      doc.changes.push({ ...(doc.changes[2] as ChangesDoc['changes'][number]), path: 'src/zz-data.sql', text: false, added: null, removed: null, copy: { size: 3_400_000, mtime: 1_700_000_000, mode: '0644', link: null } });
    });
    await goTo(r, 'zz-data.sql');
    await see(r, 'Too large to show. e opens it in your');
    // The test has no terminal to hand over: the toast names the file of the copy.
    await r.press('e');
    await see(r, 'Open it in a terminal, read-only: ~/.local/share/paddock/copies/7c1e42');
  });

  test('a conflict shows its three sides: 1 original now, 2 copy, 3 merge proposal', async () => {
    const { r } = await open();
    await see(r, 'A small shop with a cart and taxes.');
    await r.press('\r');
    await see(r, 'docs/README.md · conflict · +6 -2 · original ↔ copy');
    expect(text(r)).toContain('1 original now · 2 copy · 3 merge proposal');
    await r.press('1');
    await see(r, '· original now');
    expect(text(r)).toContain('A small shop.');
    expect(text(r)).not.toContain('A small shop with a cart and taxes.');
    await r.press('2');
    await see(r, 'conflict · +6 -2 · copy');
    expect(text(r)).toContain('A small shop with a cart and taxes.');
    expect(text(r)).not.toContain('Run it with npm start.');
    await r.press('3');
    await see(r, 'The merge of both versions is clean');
    await r.press('3');
    await see(r, 'original ↔ copy');
  });

  test('under 80 columns one pane shows at a time, and Tab changes it', async () => {
    const { r } = await open({ columns: 60, rows: 24 });
    expect(text(r)).toContain('Files · changes');
    expect(text(r)).not.toContain('docs/README.md · conflict');
    await r.press('\t');
    await see(r, 'docs/README.md · conflict');
    expect(text(r)).not.toContain('Files · changes');
    await r.press('\t');
    await see(r, 'Files · changes');
    for (const l of text(r).split('\n')) expect(l.length).toBeLessThanOrEqual(60);
  });
});

describe('keep', () => {
  test('K keeps a file in the copy, and K again takes the keep away', async () => {
    const { r, dir } = await open();
    await goTo(r, 'cart.ts');
    await r.press('K');
    await see(r, 'Kept 1 file in the copy. An apply of everything leaves it out.');
    expect(last(dir).args).toEqual(['keep', RUN, '--json', '--', 'src/cart.ts']);
    await r.waitFor((f) => /= cart\.ts/.test(strip(f)));
    expect(text(r)).toContain('src/cart.ts · kept');
    await r.press('K');
    await see(r, '1 file is part of an apply of everything again.');
    expect(last(dir).args).toEqual(['keep', RUN, '--json', '--off', '--', 'src/cart.ts']);
    await r.waitFor((f) => /~ cart\.ts/.test(strip(f)));
  });
});

describe('apply', () => {
  test('A opens the card with the files, and nothing runs before the answer', async () => {
    const { r, dir } = await open();
    await goTo(r, 'cart.ts');
    await r.press('A');
    const f = await see(r, '◆ Apply to your folder');
    expect(f).toContain('Apply · claude · webshop');
    expect(f).toContain('Write these changes to ~/code/webshop?');
    expect(f).toMatch(/▏ ~ src\/cart\.ts +changed +\+3 -2/);
    expect(f).toContain('paddock makes a backup first. u undoes this apply.');
    expect(f).toContain('[y] Apply 1 file');
    expect(f).toContain('[k] Keep in the copy');
    expect(f).toContain('[n] Cancel');
    expect(did(dir)).toEqual([]);
    // The card takes every key: j moves nothing, and A opens no second card.
    await r.press('j');
    await r.press('A');
    expect(treeRow(r, 'cart.ts')).toContain('▸');
    await r.press('y');
    await see(r, 'Applied 1 file. u undoes it.');
    expect(did(dir)).toEqual([`apply ${RUN} --on-conflict stop --token t1 --yes --json -- src/cart.ts`]);
    await gone(r, 'Apply to your folder');
    // The file has its mark until the next change.
    await r.waitFor((f2) => /✓ cart\.ts +applied/.test(strip(f2)));
  });

  test('Esc and n cancel, and Enter answers the focused button: the focus starts on Cancel', async () => {
    const { r, dir } = await open();
    await goTo(r, 'cart.ts');
    await r.press('A');
    await see(r, 'Apply to your folder');
    await r.press('\r');
    await gone(r, 'Apply to your folder');
    await r.press('A');
    await see(r, 'Apply to your folder');
    await r.press('\u001b');
    await gone(r, 'Apply to your folder');
    expect(did(dir)).toEqual([]);
    // ← from Cancel to Keep, and Enter keeps.
    await r.press('A');
    await see(r, 'Cancel: Nothing changes.');
    await r.press('\u001b[D');
    await see(r, 'Keep in the copy: An apply of everything leaves these files out.');
    await r.press('\r');
    await see(r, 'Kept 1 file in the copy');
    expect(did(dir)).toEqual([`keep ${RUN} --json -- src/cart.ts`]);
  });

  test('a folder applies its files that are ready: not the kept one', async () => {
    const { r, dir } = await open();
    await goTo(r, 'src/ ');
    await r.press('A');
    const f = await see(r, '4 files in src/');
    expect(f).not.toMatch(/▏ = src\/old\.ts/);
    expect(f).toContain('[y] Apply 4 files');
    await r.press('y');
    await see(r, 'Applied 4 files. u undoes it.');
    expect(did(dir)).toEqual([`apply ${RUN} --on-conflict stop --token t1 --yes --json -- src/cart.ts src/lib/util/format.ts src/price.ts src/tax.ts`]);
  });

  test('an apply with a delete says so, has no keep choice, and needs the second confirm', async () => {
    const { r, dir } = await open();
    await goTo(r, 'logo.png');
    await r.press('A');
    const f = await see(r, '1 file leaves your folder.');
    expect(f).toContain('[y] Apply ⚠');
    expect(f).not.toContain('Keep in the copy');
    await r.press('y');
    await see(r, 'Press y again within 3 s to confirm: Apply');
    expect(did(dir)).toEqual([]);
    // A repeat of the key is no second confirm.
    await r.press('y');
    await sleep(150);
    expect(did(dir)).toEqual([]);
    await pause();
    await r.press('y');
    await see(r, 'Applied 1 file. u undoes it.');
    expect(did(dir)).toEqual([`apply ${RUN} --on-conflict stop --token t1 --yes --json -- logo.png`]);
  });

  test('a card with more than 8 files shows 8 and counts the rest, and ↑↓ move the list', async () => {
    const { r } = await open({}, (doc) => {
      for (let i = 0; i < 12; i++) doc.changes.push({ ...(doc.changes[2] as ChangesDoc['changes'][number]), path: `src/gen/part-${String(i).padStart(2, '0')}.ts` });
    });
    await goTo(r, 'gen/');
    await r.press('A');
    const f = await see(r, '12 files in src/gen/');
    expect(f).toContain('and 4 more · ↑↓ moves the list');
    expect(f).toContain('part-07.ts');
    expect(f).not.toContain('part-08.ts');
    await r.press('\u001b[B');
    await see(r, 'part-08.ts');
    expect(text(r)).toContain('1 above · and 3 more');
  });

  test('if the original changed after the card opened, the apply stops and the card says which file', async () => {
    const { r, dir } = await open({}, undefined, (d) => {
      writeFileSync(join(d, 'stale-apply'), '');
    });
    // The list from then on: src/price.ts is another file.
    const next = JSON.parse(read(join(dir, 'changes.json'))) as ChangesDoc;
    const price = next.changes.find((c) => c.path === 'src/price.ts') as ChangesDoc['changes'][number];
    price.original = { size: 2000, mtime: 1_800_000_000, mode: '0644', link: null };
    next.token = 't7';
    writeJson(dir, 'changes-next.json', next);
    await goTo(r, 'src/ ');
    await r.press('A');
    await see(r, '[y] Apply 4 files');
    await r.press('y');
    const f = await see(r, 'Stopped: src/price.ts changed after the card opened.');
    // The card is still open, with the list of now. Nothing was written.
    expect(f).toContain('◆ Apply to your folder');
    expect(JSON.parse(read(join(dir, 'changes.json'))).changes).toHaveLength(7);
    await r.press('y');
    await see(r, 'Applied 4 files');
    expect(last(dir).args).toContain('t7');
  });

  test('an error of paddock stays on the card', async () => {
    const { r, dir } = await open({}, undefined, (d) => writeFileSync(join(d, 'fail-apply'), ''));
    await goTo(r, 'cart.ts');
    await r.press('A');
    await see(r, '[y] Apply 1 file');
    await r.press('y');
    await see(r, '✖ error: the state is locked');
    expect(text(r)).toContain('◆ Apply to your folder');
    expect(did(dir)).toHaveLength(1);
  });

  test('the palette applies all changes of the run: the card of all, after the conflict', async () => {
    const { r } = await open();
    await r.press('\u001b');
    await see(r, 'Runs · 2 live');
    await r.press(':');
    await r.press('Apply all');
    await see(r, 'Apply all changes of this run');
    await r.press('\r');
    await see(r, '◆ Both sides changed');
  });
});

describe('a conflict always asks first', () => {
  test('A on all: the conflict card comes before the Apply card, and Cancel on it applies nothing', async () => {
    const { r, dir } = await open();
    await r.press('g');
    await see(r, 'webshop/ · 7 files');
    await r.press('A');
    const f = await see(r, '◆ Both sides changed');
    expect(f).toContain('Conflict · claude · webshop');
    expect(f).toContain('Which version of docs/README.md stays in ~/code/webshop?');
    expect(f).toContain('both sides changed it');
    expect(f).toMatch(/\[o\] Keep original +\[c\] Take the copy ⚠ +\[b\] Keep both +\[m\] Take the merge/);
    // The Apply card is not there, and its key does nothing.
    expect(f).not.toContain('Apply to your folder');
    await r.press('y');
    await sleep(200);
    expect(did(dir)).toEqual([]);
    expect(text(r)).toContain('◆ Both sides changed');
    await r.press('n');
    await gone(r, 'Both sides changed');
    expect(text(r)).not.toContain('Apply to your folder');
    expect(did(dir)).toEqual([]);
  });

  test('after the choice, the Apply card shows the rest', async () => {
    const { r, dir } = await open();
    await r.press('g');
    await r.press('A');
    await see(r, '◆ Both sides changed');
    await r.press('o');
    await see(r, 'Your version stays: docs/README.md.');
    expect(did(dir)).toEqual([`apply ${RUN} --on-conflict original --token t1 --yes --json -- docs/README.md`]);
    // Now the rest: five files, one of them a delete. The conflict is not among them.
    const f = await see(r, '◆ Apply to your folder');
    expect(f).toContain('5 files');
    expect(f).not.toMatch(/▏ . docs\/README\.md/);
    expect(f).toContain('1 file leaves your folder.');
    await r.press('y');
    await pause();
    await r.press('y');
    await see(r, 'Applied 5 files. u undoes it.');
    expect(did(dir)[1]).toBe(`apply ${RUN} --on-conflict stop --token t2 --yes --json -- logo.png src/cart.ts src/lib/util/format.ts src/price.ts src/tax.ts`);
  });

  test('each choice sends its answer: keep both, the merge, and the copy after a second confirm', async () => {
    for (const [key, on, words] of [
      ['b', 'both', 'Both versions of docs/README.md'],
      ['m', 'merge', 'Applied 1 file'],
    ] as const) {
      const { r, dir } = await open();
      await r.press('A');
      await see(r, '◆ Both sides changed');
      await r.press(key);
      await see(r, words);
      expect(did(dir)).toEqual([`apply ${RUN} --on-conflict ${on} --token t1 --yes --json -- docs/README.md`]);
      cleanup();
    }
    const { r, dir } = await open();
    await r.press('A');
    await see(r, '◆ Both sides changed');
    await r.press('c');
    await see(r, 'Press c again within 3 s to confirm: Take the copy');
    expect(did(dir)).toEqual([]);
    await pause();
    await r.press('c');
    await see(r, 'Applied 1 file');
    expect(did(dir)).toEqual([`apply ${RUN} --on-conflict copy --token t1 --yes --json -- docs/README.md`]);
  });

  test('a merge with conflict marks is not offered', async () => {
    const { r } = await open({}, (doc) => {
      (doc.changes[0] as ChangesDoc['changes'][number]).merge = 'conflicts';
    });
    await r.press('A');
    const f = await see(r, '◆ Both sides changed');
    expect(f).not.toContain('Take the merge');
    expect(f).toContain('The merge has conflict marks, so paddock does not offer it.');
    await r.press('m');
    await sleep(150);
    expect(text(r)).toContain('◆ Both sides changed');
  });

  test('with more conflicts the title counts them, and each one gets its card', async () => {
    const { r, dir } = await open({}, (doc) => {
      const c = doc.changes.find((x) => x.path === 'src/price.ts') as ChangesDoc['changes'][number];
      Object.assign(c, { state: 'conflict', conflict: 'both', conflict_text: 'both sides changed it', merge: null });
      doc.counts.conflicts = 2;
    });
    await r.press('g');
    await r.press('A');
    await see(r, 'conflict 1 of 2');
    expect(text(r)).toContain('Which version of docs/README.md');
    await r.press('o');
    await see(r, 'conflict 2 of 2');
    expect(text(r)).toContain('Which version of src/price.ts');
    await r.press('b');
    await see(r, '◆ Apply to your folder');
    expect(did(dir).map((c) => c.split(' ').slice(2, 4).join(' '))).toEqual(['--on-conflict original', '--on-conflict both']);
  });
});

describe('discard', () => {
  test('D never acts without the second confirm, and it names its files', async () => {
    const { r, dir } = await open();
    await goTo(r, 'src/ ');
    await r.press('D');
    const f = await see(r, '◆ Discard from the copy');
    expect(f).toContain('5 files in src/');
    expect(f).toContain('This has no undo. Your folder does not change.');
    expect(f).toContain('[y] Discard 5 files ⚠');
    await r.press('y');
    await see(r, 'Press y again within 3 s to confirm: Discard 5 files');
    expect(did(dir)).toEqual([]);
    await r.press('n');
    await gone(r, 'Discard from the copy');
    expect(did(dir)).toEqual([]);
    await r.press('D');
    await see(r, '◆ Discard from the copy');
    await r.press('y');
    await pause();
    await r.press('y');
    await see(r, 'Discarded 5 files from the copy. Your folder did not change.');
    // Always with the paths: without one, paddock would remove the whole copy.
    expect(did(dir)).toEqual([`discard ${RUN} --yes --json -- src/lib/util/format.ts src/cart.ts src/old.ts src/price.ts src/tax.ts`]);
    expect(existsSync(join(dir, 'copy-removed'))).toBe(false);
    await gone(r, 'cart.ts');
  });

  test('the second confirm runs out after 3 s', async () => {
    const { r, dir } = await open();
    await goTo(r, 'cart.ts');
    await r.press('D');
    await see(r, '◆ Discard from the copy');
    await r.press('y');
    await see(r, 'Press y again');
    await sleep(3100);
    await r.press('y');
    await sleep(200);
    // The late key armed the choice again. It did not discard.
    expect(did(dir)).toEqual([]);
    expect(text(r)).toContain('Press y again');
  }, 30_000);
});

describe('undo', () => {
  test('u offers the last apply, and y runs paddock undo in the project folder', async () => {
    const { r, dir, project } = await open();
    await r.press('u');
    const f = await see(r, '◆ Undo the last apply');
    expect(f).toContain('before apply · 3 files · claude');
    expect(f).toContain('Bring ~/code/webshop back to the state from before it?');
    expect(did(dir)).toEqual([]);
    await r.press('y');
    await see(r, 'Undone: 2 files back, 1 removed.');
    expect(last(dir).args).toEqual(['undo', '--yes', '--json']);
    expect(last(dir).cwd).toBe(project);
  });

  test('without an apply to take back, u says so and runs nothing', async () => {
    const { r, dir } = await open({}, undefined, (d) => writeJson(d, 'backups.json', { backups: [], snapshots: [] }));
    await r.press('u');
    await see(r, 'No apply to undo.');
    expect(did(dir)).toEqual([]);
  });

  test('a newer snapshot of a run in the folder itself: the interface does not undo', async () => {
    const { r, dir } = await open({}, undefined, (d) =>
      writeJson(d, 'backups.json', {
        backups: [{ id: 'apply-1', kind: 'apply', words: 'before apply', created: 1_800_000_000, files: 3, undoable: true }],
        snapshots: [{ id: 'snap-1', kind: 'snapshot', words: 'start of run, in the folder', created: 1_800_000_500, files: 9 }],
      }),
    );
    await r.press('u');
    await see(r, 'A run in the folder itself is newer than the last apply');
    expect(did(dir)).toEqual([]);
  });
});

describe('the backups', () => {
  test('b opens the timeline by day, j moves, and R restores after the second confirm', async () => {
    const { r, dir, project } = await open();
    await r.press('b');
    const f = await see(r, 'Backups · webshop');
    expect(f).toMatch(/▸ \d\d:\d\d {2}before apply +3 files +12 KB +claude +\[R\]/);
    expect(f).toMatch(/start of run +412 files +shared +claude +\[R\]/);
    expect(f).toContain('1 of 4 · 60 KB on disk');
    await r.press('j');
    await see(r, '2 of 4');
    await r.press('G');
    await see(r, '4 of 4');
    await r.press('k');
    await see(r, '3 of 4');
    await r.press('R');
    const c = await see(r, '◆ Restore a backup');
    expect(c).toContain('before restore · 9 files');
    expect(c).toContain('[y] Restore ⚠');
    await r.press('y');
    await see(r, 'Press y again within 3 s to confirm: Restore');
    expect(did(dir)).toEqual([]);
    await pause();
    await r.press('y');
    await see(r, 'Restored: 3 files back. u undoes it.');
    expect(last(dir).args).toEqual(['restore', 'restore-20261112-172000', '--project', project, '--yes', '--json']);
    // Esc goes back to the files.
    await r.press('\u001b');
    await see(r, 'Files · changes');
  });

  test('no backup yet: the pane says when paddock makes one', async () => {
    const { r } = await open({}, undefined, (d) => writeJson(d, 'backups.json', { backups: [], snapshots: [] }));
    await r.press('b');
    await see(r, 'No backups yet. paddock makes one at the start of each run');
  });

  test('the snapshot of a run in the folder shows, and R names the command that restores it', async () => {
    const { r, dir } = await open({}, undefined, (d) => writeJson(d, 'backups.json', { backups: [], snapshots: [{ id: 'snap-1', kind: 'snapshot', words: 'start of run, in the folder', created: 1_800_000_000, files: 9, bytes: 100 }] }));
    await r.press('b');
    await see(r, 'start of run, i');
    await r.press('R');
    await see(r, 'paddock undo ID restores it');
    expect(did(dir)).toEqual([]);
  });
});

describe('the pointer', () => {
  test('i opens the card with the store, its size and the disk, and Esc closes it', async () => {
    const { r } = await open();
    await r.press('i');
    const f = await see(r, 'encrypted (LUKS)');
    expect(f).toContain('Pointer · ~/code/webshop');
    expect(f).toContain('● linked   The pointer and the store agree.');
    expect(f).toMatch(/Store +~\/\.local\/share\/paddock\/copies\/7c1e42 {2}· {2}1 copy {2}· {2}4 backups/);
    expect(f).toMatch(/Size +60 KB of its own/);
    expect(f).toContain('A copy of this folder carries only the pointer, not the files.');
    expect(f).toContain('[Esc] Close');
    await r.press('\u001b');
    await gone(r, 'Pointer · ~/code/webshop');
    expect(text(r)).toContain('Files · changes');
  });

  test('a folder without a pointer: ▲ and the word, and c creates it', async () => {
    const { r, dir, project } = await open({}, undefined, (d) => {
      const linked = read(join(d, 'pointer.json'));
      writeFileSync(join(d, 'pointer-after.json'), linked);
      writeJson(d, 'pointer.json', { state: 'no pointer', text: 'The folder has no .paddock/pointer.json.', action: 'paddock pointer create', ok: false, pointer: null, store: null, encryption: { state: 'not_encrypted' } });
    });
    await r.press('i');
    const f = await see(r, '▲ no pointer');
    expect(f).toContain('The folder has no .paddock/pointer.json.');
    expect(f).toMatch(/Disk +not encrypted/);
    expect(f).toContain('[c] Create it');
    await r.press('c');
    await see(r, '● linked');
    expect(did(dir)).toEqual([`pointer create ${project}`]);
    expect(text(r)).not.toContain('[c] Create it');
  });

  test('a copy of a project folder: a new store needs the second confirm', async () => {
    const { r, dir, project } = await open({}, undefined, (d) => writeJson(d, 'pointer.json', { state: 'copy', text: 'This folder is a copy of another project folder.', action: 'paddock pointer new', ok: false, store: { dir: '/x/store', original: '/other/webshop', copies: 2, backups: 5, size: { bytes: 4096 } }, encryption: { state: 'unknown' } }));
    await r.press('i');
    const f = await see(r, '▲ copy');
    expect(f).toContain('[n] New store for this folder ⚠');
    expect(f).toMatch(/Belongs to +\/other\/webshop/);
    await r.press('n');
    await see(r, 'Press n again within 3 s to confirm');
    expect(did(dir)).toEqual([]);
    await pause();
    await r.press('n');
    await until(() => did(dir).length === 1);
    expect(did(dir)).toEqual([`pointer new ${project} --yes`]);
  });
});

describe('read-only', () => {
  test('all screens show, and each action key answers with the toast and runs nothing', async () => {
    const { r, dir } = await open({ readonly: true });
    await goTo(r, 'cart.ts');
    await see(r, '@@ line 38');
    for (const k of ['A', 'K', 'D', 'u']) {
      await r.press(k);
      await see(r, 'read-only: no changes');
      expect(text(r)).not.toContain('◆ ');
      await r.press('j');
      await r.press('k');
    }
    await r.press('\r');
    await see(r, 'original ↔ copy');
    await r.press('\u001b');
    await r.press('b');
    await see(r, 'Backups · webshop');
    await r.press('R');
    await see(r, 'read-only: no changes');
    expect(text(r)).not.toContain('Restore a backup');
    await r.press('i');
    await see(r, 'encrypted (LUKS)');
    expect(did(dir)).toEqual([]);
    // The footer names no action.
    expect(text(r).split('\n').at(-1)).not.toMatch(/apply|discard|restore/);
  });

  test('read-only hides the action of a pointer that is not linked', async () => {
    const { r, dir } = await open({ readonly: true }, undefined, (d) => writeJson(d, 'pointer.json', { state: 'no pointer', text: 'The folder has no .paddock/pointer.json.', action: 'paddock pointer create', ok: false }));
    await r.press('i');
    const f = await see(r, '▲ no pointer');
    expect(f).not.toContain('Create it');
    await r.press('c');
    await sleep(150);
    expect(did(dir)).toEqual([]);
  });
});

describe('the states of the copy', () => {
  test('no change yet, all applied, and a filter without a match', async () => {
    const { r } = await open({}, (doc) => {
      doc.changes = [];
      doc.counts = { new: 0, changed: 0, deleted: 0, conflicts: 0, kept: 0, ready: 0, applied: 0, behind: 0 };
    });
    await see(r, 'No changes yet.');
    expect(text(r).replace(/[│\s]+/g, ' ')).toContain('The agent works in its copy.');
    expect(text(r)).toContain('copy: no open change');
    await r.press('c');
    await r.press('c');
    await see(r, 'No conflicts.');
    cleanup();
    const b = await open({}, (doc) => {
      doc.changes = [];
      doc.applied = ['src/cart.ts'];
      doc.counts = { new: 0, changed: 0, deleted: 0, conflicts: 0, kept: 0, ready: 0, applied: 1, behind: 0 };
    });
    await see(b.r, '✓ cart.ts');
    await b.r.press('g');
    await see(b.r, 'All changes are in your folder. u undoes');
  });

  test('a file that the agent writes at this moment has the word writing', async () => {
    const { r } = await open({}, (doc) => {
      (doc.changes.find((c) => c.path === 'src/price.ts') as ChangesDoc['changes'][number]).writing = true;
    });
    await r.waitFor((f) => /~ price\.ts +writing/.test(strip(f)));
  });

  test('a direct run has no tree: the text says that there is no copy', async () => {
    const dir = fakeWorld((d) => {
      demoCopy(d);
      // paddock knows no copy of this run.
      writeJson(d, 'copies.json', { runs: [] });
      unlinkSync(join(d, 'changes.json'));
    });
    void dir;
    const r = await renderApp({ columns: 80, rows: 24 });
    await see(r, 'npm test');
    await r.press('f');
    await see(r, 'This run works in your folder directly. There is no copy.');
    // The backups of the project are still one key away.
    await r.press('b');
    await see(r, 'Backups · webshop');
  });

  test('a copy that was removed: the backups are still here', async () => {
    const { r } = await open({}, (doc) => {
      doc.run.state = 'removed';
      doc.changes = [];
    });
    await see(r, 'The copy is gone. The backups are still here: press b.');
    await r.press('b');
    await see(r, 'before apply');
  });

  test('an ended run: the room line says ended, and the actions work', async () => {
    const { r, dir } = await open({}, (doc) => {
      doc.run.state = 'ended';
    }, (d) => {
      writeJson(d, 'ps.json', { sandboxes: [], ended: [{ id: RUN, short: '7c1e42', harness: 'claude', project: join(d, 'code', 'webshop'), ended: 60, status: 'ok', text: 'done' }] });
    });
    await see(r, 'claude · webshop · ended · copy:');
    await goTo(r, 'cart.ts');
    await r.press('A');
    await see(r, '[y] Apply 1 file');
    await r.press('y');
    await see(r, 'Applied 1 file');
    expect(did(dir)).toHaveLength(1);
  });

  test('a live update changes the tree, and the selection stays on its file', async () => {
    const { r, dir } = await open();
    await goTo(r, 'price.ts');
    const doc = JSON.parse(read(join(dir, 'changes.json'))) as ChangesDoc;
    doc.changes.unshift({ ...(doc.changes[2] as ChangesDoc['changes'][number]), path: 'aaa/first.ts', kind: 'new' });
    doc.counts.new += 1;
    writeJson(dir, 'changes.json', doc);
    await see(r, 'first.ts');
    expect(treeRow(r, 'price.ts')).toContain('▸');
    expect(text(r)).toContain('src/price.ts · changed');
    expect(text(r)).toContain('copy: 4 changed, 3 new, 1 deleted');
  });

  test('the text of the agent reaches the screen without its escape codes', async () => {
    const { r } = await open({}, (doc) => {
      doc.changes.push({ ...(doc.changes[2] as ChangesDoc['changes'][number]), path: 'src/ev\u001b[31mil\u0007.ts' });
    });
    await see(r, 'evil.ts');
    expect(r.frame()).not.toContain('\u0007');
    expect(r.frame()).not.toContain('\u001b[31mil');
  });
});

describe('the mouse', () => {
  test('a click selects a row, and a second click on the selected file opens the diff', async () => {
    const { r } = await open();
    const p = at(r, 'price.ts');
    await r.press(click(p.x + 2, p.y));
    await see(r, 'src/price.ts · changed');
    expect(treeRow(r, 'price.ts')).toContain('▸');
    expect(text(r)).toContain('Files · changes');
    await sleep(500);
    await r.press(click(p.x + 2, p.y));
    await see(r, 'src/price.ts · changed · +8 -8 · original ↔ copy');
    expect(text(r)).not.toContain('Files · changes');
  });

  test('a click on ▾ closes the folder, and a click on ▸ opens it', async () => {
    const { r } = await open();
    const d = at(r, '▾ src/');
    await r.press(click(d.x, d.y));
    await r.waitFor((f) => !inTree('cart.ts')(f));
    await sleep(500);
    const c = at(r, '▸ src/');
    await r.press(click(c.x, c.y));
    await r.waitFor(inTree('cart.ts'));
  });

  test('a click on [A], [K] and [D] of the selected row does the same as the key', async () => {
    const { r, dir } = await open();
    await goTo(r, 'cart.ts');
    const k = at(r, '[K]');
    await r.press(click(k.x + 1, k.y));
    await see(r, 'Kept 1 file in the copy');
    expect(did(dir)).toEqual([`keep ${RUN} --json -- src/cart.ts`]);
    const a = at(r, '[A]');
    await r.press(click(a.x + 1, a.y));
    await see(r, '◆ Apply to your folder');
    await r.press('n');
    await gone(r, 'Apply to your folder');
    const d = at(r, '[D]');
    await r.press(click(d.x + 1, d.y));
    await see(r, '◆ Discard from the copy');
  });

  test('the buttons show on the row below the pointer, and a click on them acts on that row', async () => {
    const { r } = await open();
    const p = at(r, 'tax.ts');
    // The pointer moves over the row: its word gives way to the buttons.
    await r.press(`\u001b[<35;${p.x + 3};${p.y + 1}M`);
    await r.waitFor((f) => /\+ tax\.ts +\[A\]\[K\]\[D\]/.test(strip(f)));
    const row = rowOf(r, 'tax.ts');
    const x = Array.from(row.slice(0, row.indexOf('[D]'))).length;
    await r.press(`\u001b[<35;${x + 2};${p.y + 1}M`);
    await r.press(click(x + 1, p.y));
    const f = await see(r, '◆ Discard from the copy');
    expect(f).toMatch(/▏ \+ src\/tax\.ts/);
  });

  test('the wheel moves the tree under the pointer, and the diff under the pointer', async () => {
    const { r } = await open();
    await goTo(r, 'cart.ts');
    await see(r, '@@ line 38');
    const t = at(r, 'README.md');
    await r.press(wheel(t.x, t.y, true));
    await r.waitFor((f) => !/▸ +~ cart\.ts/.test(strip(f)));
    await goTo(r, 'cart.ts');
    const d = at(r, '@@ line 38');
    await r.press(wheel(d.x + 4, d.y + 2, true));
    await r.press(wheel(d.x + 4, d.y + 2, true));
    await r.waitFor((f) => !strip(f).includes('@@ line 38'));
    expect(treeRow(r, 'cart.ts')).toContain('▸');
  });

  test('a click on a footer key does the same as the key', async () => {
    const { r } = await open();
    const b = at(r, 'b backups');
    await r.press(click(b.x + 3, b.y));
    await see(r, 'Backups · webshop');
    const e = at(r, 'Esc back');
    await r.press(click(e.x + 1, e.y));
    await see(r, 'Files · changes');
    const c = at(r, 'c filter');
    await r.press(click(c.x + 2, c.y));
    await see(r, 'Files · ready to apply');
  });

  test('a click on a card button focuses it, a second click answers, and a ⚠ choice still needs its confirm', async () => {
    const { r, dir } = await open();
    await goTo(r, 'cart.ts');
    await r.press('D');
    await see(r, '◆ Discard from the copy');
    const y = at(r, '[y] Discard 1 file');
    await r.press(click(y.x + 4, y.y));
    await see(r, 'Discard 1 file: The copy loses these changes for good.');
    expect(did(dir)).toEqual([]);
    expect(text(r)).not.toContain('Press y again');
    await sleep(500);
    await r.press(click(y.x + 4, y.y));
    await see(r, 'Press y again within 3 s to confirm');
    expect(did(dir)).toEqual([]);
    await sleep(500);
    await r.press(click(y.x + 4, y.y));
    await see(r, 'Discarded 1 file from the copy');
    expect(did(dir)).toEqual([`discard ${RUN} --yes --json -- src/cart.ts`]);
  });

  test('a click outside a card closes nothing: only Esc or Cancel does', async () => {
    const { r, dir } = await open();
    await goTo(r, 'cart.ts');
    await r.press('A');
    await see(r, '◆ Apply to your folder');
    // On a row of the tree under the card, on the diff, and on the empty state row.
    const p = at(r, '! README.md');
    await r.press(click(p.x + 2, p.y));
    await sleep(500);
    await r.press(click(60, p.y));
    await sleep(500);
    await r.press(click(40, 22));
    await sleep(150);
    expect(text(r)).toContain('◆ Apply to your folder');
    // The selection under the card did not move.
    expect(treeRow(r, 'cart.ts')).toContain('▸');
    const n = at(r, '[n] Cancel');
    await r.press(click(n.x + 2, n.y));
    await sleep(500);
    await r.press(click(n.x + 2, n.y));
    await gone(r, 'Apply to your folder');
    expect(did(dir)).toEqual([]);
  });

  test('a click on [R] opens the Restore card, and a click on the pointer word opens the Pointer card', async () => {
    const { r } = await open();
    await r.press('b');
    await see(r, 'Backups · webshop');
    const row = rowOf(r, 'before restore');
    const y = text(r).split('\n').indexOf(row);
    await r.press(click(Array.from(row.slice(0, row.indexOf('[R]'))).length + 1, y));
    const f = await see(r, '◆ Restore a backup');
    expect(f).toContain('before restore · 9 files');
    await r.press('n');
    await gone(r, 'Restore a backup');
    const p = at(r, 'pointer: ● linked');
    await r.press(click(p.x + 4, p.y));
    await see(r, 'encrypted (LUKS)');
  });
});
