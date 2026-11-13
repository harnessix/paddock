# paddock-tui

The terminal interface of [paddock](https://github.com/harnessix/paddock).
It shows every run, the approvals that wait for you, the mail of the runs
and the board in one terminal window. You can use it with the keyboard
alone or with the mouse.

## Start it

Run `paddock` alone in a terminal. The interface opens on its home screen.
`paddock tui` opens the interface on the runs. In a pipe, a script or CI,
`paddock` alone prints its help, and the interface does not start.

paddock starts the first of these that it finds:

1. The program that `PADDOCK_TUI` names, as an absolute path. This is for
   development.
2. A `paddock-tui` program in the folder of the paddock binary.
3. The copy of its own version that paddock keeps in its data folder
   (`~/.local/share/paddock/tui/VERSION/paddock-tui`).
4. `paddock-tui` on your PATH.

You do not build or install the program by hand. A paddock release holds it
next to `paddock`, and `paddock update` installs both. `bin/install`, the
Arch package and the Homebrew formula build it together with paddock. If
it is missing, the first `paddock tui` or `paddock` alone downloads the one
of paddock's own version from the GitHub release, checks its checksum and
its version, and starts it. `paddock help install` has the details, and the
cases in which paddock does not download. In those cases, `paddock tui`
prints one line with the command to run. `paddock` alone prints the same
line and then its help.

paddock does not use `npx`: npx downloads code from npm, and it also runs a
package or reads an `.npmrc` from the current folder, which an agent can
write. The package is not on npm. Do not install a package with this name
from npm, because the project does not own the name there.

## The home screen

The home screen has the paddock icon, the version and one line of state,
for example "2 agents working · 1 request waits for you". Under it are the
things to do. Each has a key, and a click opens it too.

| Key | Choice | What it opens |
| --- | --- | --- |
| `n` | New agent | Pick an agent, a folder and the options, then start. |
| `o` | Open an app | The desktop apps that paddock knows. Enter runs `paddock app run NAME`. |
| `a` | Requests | The requests that wait for your answer, with their count. |
| `r` | Runs | Every run. |
| `w` | World | The board. |
| `s` | Settings | `paddock setup`, `paddock doctor` and `paddock help`, each in the terminal. |
| `?` | Help | Every key. |

`H` opens the home screen from any screen. After `paddock` alone, Esc on a
first screen goes back to it, and Esc on the home screen quits. With
`--readonly`, New agent, Open an app and Settings do not show.

### New agent

1. Pick an agent. The list has the installed agents of `paddock list`. The
   character of the agent shows beside the choices. The first pick is the
   agent that ran last in the current folder.
2. Pick a folder: the current folder, a folder of `paddock recent`, or a
   path that you type. `~` is your home folder.
3. Read what the folder guard says. paddock checks the folder with
   `paddock explain --json` before the start. If paddock refuses the
   folder, the reason shows and Start does nothing. If the folder is wide,
   for example a folder of many projects, Start needs your yes first.
4. Set the options. Network: Default, Model only, Internet, No network or
   This computer. Approvals: Auto or Manual. Branch: work in a new git
   worktree on its own branch.
5. Press Enter. The interface leaves the screen, and `paddock run` runs in
   the terminal with the agent. When the run ends, the home screen comes
   back. If the run fails, its last lines stay until you press Enter.

If no agent is installed, New agent lists the agents that paddock knows
and how to install them. Install one, then press `R`.

Tiles need 80 columns and 18 rows. A smaller terminal and `--plain` show
the choices as a list. With `NO_COLOR` the icon is the fence in blocks.

## Options

| Option | What it does |
| --- | --- |
| `--plain` | No boxes, no colors, no mouse and no alternate screen. Use it with a screen reader or for logs. |
| `--readonly` | Hide every action that changes something. |
| `--no-mouse` | Start with the mouse off. `PADDOCK_TUI_MOUSE=0` does the same. |
| `--reduce-motion` | The world shows each change without motion. `PADDOCK_TUI_MOTION=reduce` does the same. |
| `--theme dark` or `--theme light` | The color theme. The default comes from `PADDOCK_TUI_THEME`, then `COLORFGBG`, then dark. |
| `--home` | Start on the home screen. `paddock` alone does this. |

`paddock` alone takes the same options, for example `paddock --plain`.

## Keys

Press `?` for every key and `:` or `Ctrl+P` for the command palette.
The footer always shows the main keys of the current screen.

While the mouse is on, the terminal does not select text on a plain drag.
Hold Shift to select text, or press `M` to turn the mouse off.

Every key counts, also when several arrive at once (fast typing, a key that
repeats, a slow connection): the interface takes them one by one, in their
order. A paste goes into the open text field, with its line breaks as
spaces. With no text field open, a paste does nothing: its letters are never
commands. The question before a stop takes no answer from keys typed ahead.

A button shows a press at once (inverse video) and acts on the release. A
room, a row or a button under the pointer is marked. `Ctrl+L` draws the
whole screen again, for when another program wrote over it. `R` does the
same and reads paddock again.

## Rooms

The runs show as rooms: one card per run, with its agent as a small
character at a desk, the agent and project, the state word and the time in
the state. The character shows the state at a glance: it works, waits for
you, idles, sleeps (ended or frozen) or is blocked (failed or stuck). Each
agent has its color: orange claude, blue codex, purple gemini, green copilot,
pink grok, teal opencode, gray for any other.

The rooms need 80 columns and 24-bit or 256 colors. Under 80 columns, with
`--plain`, with `NO_COLOR` or in a terminal with only 16 colors, the runs show
as the list. `L` switches between the rooms and the list.
`PADDOCK_TUI_ART=off` turns the art off; `PADDOCK_TUI_ART=256` or
`truecolor` sets the colors when the terminal does not say.

The characters move at 8 frames a second while the terminal has the focus.
Each step writes only the cells that change. In the background, or when the
terminal does not show the card, nothing moves.

## The ranch

The Board tab (`b`) shows the board as a world. The ranch holds all projects.
A building is one project. A room is one run. The loft is the attic of a
building, and it holds the private notes of the project.

The wall of a room is the sandbox. Everything that leaves the room is a line
through the wall. Amber has one meaning: the thing touches your real machine,
or it waits for you. Nothing else is amber.

| Key | What it does |
| --- | --- |
| Arrow keys | Select the next thing in that direction. |
| `Enter` | Zoom in: ranch, building, room. On the door of a room, open the request card. On the mailbox, open the messages. |
| `Esc` | Go one stop out. At the ranch, go back to the runs. |
| `1` `2` `3` | Jump to the ranch, the building, the room. |
| `l` | Go up to the loft of the project. |
| `P` | Turn the Plumbing view on or off. |
| `I` | Open or close the Inspector. |
| `c` | Copy the command of the selected thing. |
| `z` | Pause or resume the selected room. |
| `L` | Show the board as a list, and the ranch again. |

The ranch is one green island that fills the screen. Each project is a house
with its name on a sign under it. Each run is a room of the house, with the
name of the run under it. A helper has its own hut at the end of a path. A
shared yard is a fence in front of its rooms. The build bench and the ranch
gate stand at the front edge, and another computer is an island in a bay. A
project without a run is a closed house: its light is off and its door is
shut. The tickets of a project are on its sign: select the sign to read them
in the line at the bottom, or in the Inspector.

Words do not lie on the pictures. A name stands on a sign or under its thing.
A name that does not fit ends with an ellipsis.

The pictures fit the screen. From 160 columns on, each pixel of the ranch is
twice as large if the whole ranch fits so. A building and a room stand in the
middle of the screen, and from 150 columns on they are twice as large too. At
80 columns the ranch is compact: low roofs, and a rail under the rooms of a
yard. A ranch that is too high for the screen scrolls by rows.

The request card opens beside the door, in the picture. Its answers and its
keys are those of the Approvals tab. `Esc` closes the card and gives no
answer.

The simple view is the default. It shows the rooms, the characters, the door
with a request, the window, the mailbox flag and frost on a paused room. It
also shows everything amber, for example the cable of a tool that runs on your
machine. The Plumbing view adds glass walls, every tool with its cable, the
shared notes and the labels.

The world uses plain words: "Tools", "Internet", "Shared yard", "Messages".
The Inspector shows the paddock word for the selected thing, the exact
command behind it and the JSON that the picture was drawn from. It also
names what paddock does not report yet.

The window of a room shows its network mode. Closed shutters mean no network.
One pipe means only the service of the model. A grille means a list of hosts.
An open window means the whole internet. An amber door means the network of
your machine.

The world moves at 8 frames a second. A character walks to the door with its
request, and walks back after your answer. Frost grows on a paused room. The
flag of the mailbox goes up for unread mail. The window turns into its new
mode. A spark runs along the cable of a tool that starts. At a socket with a
cover, the spark stops and a card rises. A tag flips when a shared note is
written. The room of a helper builds at the end of its path, and an envelope
travels back when the helper ends. The fan of another computer turns while a
job runs there.

The camera moves too. `Enter` opens a building over the ranch, from the room
that you opened. `Esc` closes it on that room again. The loft comes down from
the top. With `P` the walls turn to glass.

If nothing changes, no timer runs. If the terminal is in the background,
nothing moves, and a motion goes on where it was when you come back.

`--reduce-motion` or `PADDOCK_TUI_MOTION=reduce` turns the motion off. Then
each move of the camera is one short fade of 150 ms. The walk, the frost and
the window show their end state at once, and a paused room has a snowflake
badge. A call of a tool makes its cable bright for one second. A written note
shows a dot for two seconds. The fan is a lit window. The interface also reads
the Reduce Motion setting of macOS, of GNOME and of KDE, but not over SSH.
`PADDOCK_TUI_MOTION=full` keeps the motion on. Without art (`NO_COLOR`,
`--plain`, 16 colors) nothing moves at all.

Three motions wait for facts that paddock does not report yet. The mailbox
hops once for a question. A character climbs the ladder, writes a note in the
loft, and a new notebook slides onto the shelf. A spark runs for each call of
a tool, not only when the tool starts.

The ranch is the default of the Board tab in a terminal with 80 columns and
24-bit or 256 colors. With `NO_COLOR`, with `--plain` or with only 16 colors,
the Board tab shows the list. There, `L` shows the ranch as text without any
color code. The world needs 60 columns and 11 rows.

The pictures of the world are the pixel art of the launch video scene
(`sprites/world`, see "Sprites"). In the room of a paused run the walls are
ice. In the room of an ended run the light is off. With `P` the walls are
glass.

## The files of a run

A run with `--copy` works on a copy of the project. Your folder changes
only when you apply. The `files` tab of a run shows what the agent changed
in its copy, and you apply, keep or discard each change there.

To open the tab, press `f` on a run, or Tab on the run screen. In the
rooms, a card with open changes has the line `◆ 7 changes · 1 conflict`,
and a click on that line opens the tab. In the ranch, press Enter on the
model on the desk of a room. The model glows amber while changes wait
for you.

The left pane is the tree of the changed files. The right pane is the
diff of the selected file. Each file has a mark and a word:

| Mark | Word | Meaning |
| --- | --- | --- |
| `+` | new | The agent made the file. |
| `~` | changed | The agent changed the file. |
| `-` | deleted | The agent deleted the file. |
| `!` | conflict | You and the agent both changed the file. |
| `=` | kept | The file stays in the copy when you apply all. |
| `✓` | applied | The change is in your folder. |

The first row of the tree is the project folder, and it stands for all
changes. A folder row acts on all files in the folder.

| Key | Action |
| --- | --- |
| `↑` `↓` `j` `k` `g` `G` | Move in the tree. |
| `←` `→` | Close and open a folder. |
| Enter | Give the diff the whole width. On a folder: open or close it. |
| Tab | Put the focus on the tree or on the diff. |
| `A` | Apply the file or the folder to your folder. |
| `K` | Keep the file in the copy. Press `K` again to take the keep away. |
| `D` | Discard the change from the copy. |
| `u` | Undo the last apply. |
| `c` | Change the filter: changes, ready to apply, conflicts. |
| `/` | Filter by name. |
| `n` `N` | Go to the next and to the previous change of the diff. |
| `t` | Show the diff unified or side by side. Side by side needs 120 columns. |
| `e` | Open the file of the copy in `$EDITOR`, read-only. |
| `b` | Show the backups of the project. `R` restores the selected backup. |
| `i` | Show the pointer state: the store of the copies and backups. |

Each action runs a paddock command (`paddock apply`, `keep`, `discard`,
`undo`, `restore`), and the result shows in the line above the footer.
An action that writes to your folder or deletes a change opens a card
first. A choice with `⚠` needs the same key again within 3 seconds. This
applies to a discard, a restore, an apply that deletes files, and "Take
the copy" on a conflict.

paddock never overwrites a conflict. If you apply a folder that holds a
conflict, a card asks about each conflict first: keep the original, take
the copy, keep both, or take the merge. The Apply card for the other
files opens after your choice. If you cancel a conflict card, nothing is
applied. In the diff of a conflict, `1`, `2` and `3` show the original,
the copy and the state of the merge.

If a file of your folder changes while a card is open, the apply stops,
and the card names the file.

With the mouse, a click selects a row, and a second click opens the diff.
The buttons `[A]`, `[K]` and `[D]` show on the selected row and on the
row under the pointer. The wheel scrolls the pane under the pointer. A
click outside a card does not close the card.

After an apply, the ranch shows the changed parts as they move from the
desk into the house. With reduced motion, nothing moves.

With `--readonly`, all screens show, and each action key answers
"read-only: no changes".

## Colors and small terminals

The colors are the 16 colors of the terminal, so its light or dark palette
decides the shades. No state shows by color alone: each has a glyph and a
word. With `NO_COLOR` the interface writes no color code at all; bold, dim,
inverse and underline carry the roles, and the runs show as the list.
`--plain` has no boxes, no colors and no mouse.

Under 80 columns the runs show as the list. Under 43 columns a row shows the
state in one word, the agent and where it runs. The header then has only the
tabs. A pane that is too low for its lines cuts them at the bottom.

## Languages

The interface speaks English, Simplified Chinese, Traditional Chinese,
Japanese, Korean, Spanish, French, German, Brazilian Portuguese, Russian and
Italian. The same set as paddock for Mac. The language comes from
`PADDOCK_LANG`, then `LC_ALL`, `LC_MESSAGES` and `LANG`. The first variable
that is set decides. `C`, `POSIX` and a language that is not in the list
give English. For example, `PADDOCK_LANG=ja paddock tui` starts the
interface in Japanese.

Commands, flags, paths, ids, keys and the names of agents stay as they are
in every language. CJK text takes two cells per character. A text that does
not fit its column is cut with an ellipsis, so the boxes stay aligned.

The texts are in `src/i18n/`. `en.ts` has every key and its English text,
and `locales/` has one file per language with the same keys. A text with a
count has plural forms, and `Intl.PluralRules` picks the form. To add a
text, add its key to `en.ts` and to every file in `locales/`.
`test/i18n.test.ts` makes sure that every language has every key with the
same placeholders. `test/l10n-views.test.tsx` keeps the main views in
English, German, Japanese and Simplified Chinese at 80x24 and 120x40 as
snapshots in `test/__l10n__/`.

## Drawing

The interface keeps the cells that the terminal shows. For a new frame it
writes only the cells that differ, each run of cells with one cursor move
and the shortest color codes. It clears the screen once at the start and
after a resize, and it never erases a line, so the screen does not flicker.
Frames that come faster than every 8 ms are joined, and a terminal that is
slow gets only the last frame. Where the terminal knows synchronized output
(mode 2026), a frame shows all at once.

While nothing changes, nothing happens: a read of paddock with the same
answer as before draws nothing, and a time such as "12 s" or "4 min" wakes
the interface only when its word changes. Measured with 20 rooms on a
200 x 60 terminal: no byte and about 0.3 % of one processor while the
terminal is in the background (the reads of paddock), about 3 % with the
animation, and about 10 ms from a key to its frame.
`PADDOCK_TUI_STATS=1 npx vitest run test/perf.test.tsx` prints the numbers
of the test with a fake clock.

### Sprites

The characters are the toon sprites of the launch video
(`sprites/frames.json`). A sprite is `sprites/character-COLOR-STATE.json`:
COLOR is `orange` (claude), `blue` (codex), `purple` (gemini), `green`
(copilot), `pink` (grok), `teal` (opencode) or `gray` (any other agent),
STATE is `idle`, `work` or `wait` (8 frames each), `sleep` or `blocked`
(4 frames each). The file holds
`{"w":24,"h":24,"fps":8,"frames":[[[r,g,b] or null per pixel, row by row]]}`;
`null` is a transparent pixel. Each terminal cell shows two pixels, so a
sprite takes 24 columns and 12 rows. All loops play at 8 frames a second.

The world has its own layers in `sprites/world`, in the same file form:
the small characters of the ranch (8 x 8), the shells of the rooms (lit,
night, ice and glass), the windows, the door, the furniture and the tiles.
`sprites/world/frames.json` lists each layer with its size, its frame count
and its frame rate. It also gives the cuts of a room shell: the corners keep
their size, and the rest stretches. A loop shows each frame for a whole
number of the 8 steps of a second.

The build puts the 35 character files and the layers of the world into the
program, also into the single binary. It stops when a file is missing or has
the wrong size or frame count. At run time a layer of the world that does not
load shows as a placeholder that the program draws (`src/world/pixels.ts`),
and one line says so, once (on the standard error, when that is not the
terminal). The pictures that the
world makes from a layer (a room shell at a size, a strip of a tile) are made
once and kept, and so is the cell of each pair of colors. At run time a color whose file does not load gets the gray
sprite of the state, and without that a built-in gray character. The cells
and their color codes are made once per sprite, when it first shows; an
animation step only writes the ready text of the changed cells.

## How it works

The interface reads and changes everything through paddock's commands.
It runs `paddock ps --json`, `paddock board --json`, `paddock approvals --json`,
`paddock mail ls --json` and `paddock view ID --tail --json`. Actions run
`paddock approvals answer`, `paddock run --detach`, `paddock stop`,
`paddock view`, `paddock split` (when paddock has it) and `paddock mail send`.
The ranch adds five reads, and only while it is on the screen:
`paddock device ls --json`, `paddock kv overview --json`,
`paddock files --json`, `paddock forage --json` and
`paddock horse list --json`. The pause key runs `paddock freeze` and
`paddock thaw`.
`PADDOCK_BIN` selects the paddock binary. `paddock tui` sets it.

## Develop

The tools come from `mise.toml` (Node.js 22 and bun).

```sh
cd tui
npm ci
npm run check                        # typecheck, lint and all tests
node scripts/compile.mjs linux-x64   # bin/paddock-tui-linux-x64
PADDOCK_TUI_UNDER_TEST=$PWD/bin/paddock-tui-linux-x64 npx vitest run test/pty.test.ts
```

A development build of paddock downloads no interface. Use one of these:

```sh
PADDOCK_TUI=$PWD/dist/cli.js paddock tui   # the build of this folder, after npm run build
scripts/install.sh                         # one program, next to the paddock on PATH
```

`mise.lock` pins node and bun with the checksum of each download. The CI
and the release run mise with `MISE_LOCKED=1`, so a tool that the file does
not pin fails the build. After a change of a version in `mise.toml`, run
`mise lock --platform linux-x64,linux-arm64,macos-arm64`.

The tests use a fake paddock (`test/fake-paddock.mjs`) that answers from
the JSON files in `test/fixtures` and records every call.
