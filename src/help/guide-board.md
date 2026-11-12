## Board: every project, ticket and run

`paddock board` shows every project, ticket and agent run on one screen. It answers one question: which work runs now, and which work waits for you.

```sh
paddock board
paddock board --watch
paddock board --json
```

The top lines show the approval cards that wait for you and the compile slots that runs hold. Below them, each project has one block. Each ticket has one line: a status glyph, the state, the ticket ID, the title, the run IDs and the last status. The most urgent line comes first, and a run without a ticket takes its place among the tickets by its state. Each line is cut to the terminal width. `--watch` draws the board again when it changes. `--json` prints the same data for the Omarchy panel. With `--watch`, it prints one line of JSON for each change.

### Projects and tickets

A project is the repository of a run. A worktree counts as its main repository. A project shows only while it has a run on the board, so a project whose open tickets have no run in the last 7 days does not show. The tickets of a project are the files `.paddock/tickets/*.md` in its private notes (see `paddock help private-notes`).

- The ticket ID is the file name before the first `-` or `.`. For example, the ID of `KV2-worker.md` is `KV2`.
- The title is the first heading of the file, without a leading `KV2:`.
- A file with a line that starts with `Status:` shows as a ticket, also when no run has it. Other files show only when a run has them.

A run belongs to a ticket when its prompt file is that ticket. paddock records the prompt file when the run starts, with `paddock run --detach --prompt-file FILE` or with `paddock-host spawn --prompt-file FILE`. If the prompt names no file, a ticket with the same text matches. A run without a ticket shows under its project, marked `(no ticket)`.

The `file` of a ticket in `--json` is the path in the private notes, relative to the project: `.paddock/tickets/KV2-worker.md`. When the project has a loft, the file can be only in the loft's `tree/.paddock/tickets/`. A reader that opens the file tries the loft first and then the project: when both have a file of the same name, the loft copy wins, as on the board. The `file` value and the project `path` are exact, so a reader can open them.

Text that a run or a ticket chooses does not reach the terminal as an escape sequence or as an invisible character. The board removes control characters, escape sequences, and bidi and zero-width characters from the ticket ID, title and `Status:` line, the run ID, harness, model and last line, the project name and the compile slot holder. The text view also shows the project path without them. In `--json`, the exact `file` and `path` values can hold such characters. paddock writes DEL, U+0080 to U+009F and the invisible characters of the list below as `\uXXXX` escapes, so a JSON reader gets the same value and a terminal shows only plain text.

When the project has a loft folder, the project in `--json` has the field `loft`. It is the absolute notes folder of the loft, for example `/home/demo/.local/share/paddock/loft/KEY/tree/.paddock`, with or without a `.paddock` link in the project. A loft-only ticket file is then at `<loft>/tickets/KV2-worker.md`. If the loft has no such folder, the field is absent. If the path is not plain, the field is absent too and paddock prints a warning: the path is never cleaned, because a cleaned path names another folder. The board and its panel use the same rule, so they list and open the same tickets. A path is not plain when it is longer than 1024 characters (code points: an emoji counts as one), or when it has one of these characters:

- a control character: U+0000 to U+001F and U+007F to U+009F
- U+00AD (soft hyphen), U+061C (Arabic letter mark) and U+180E (Mongolian vowel separator)
- U+200B to U+200F (zero-width characters and left-to-right and right-to-left marks)
- U+2028 and U+2029 (line and paragraph separators)
- U+202A to U+202E (bidi embeddings and overrides)
- U+2060 to U+206F (word joiner, invisible operators, bidi isolates and other format characters)
- U+FEFF (byte order mark)
- U+FFF9 to U+FFFB (interlinear annotation characters)

The field is optional and new in version 1, so a reader must ignore fields that it does not know.

### States

The states, the most urgent first:

| Glyph | State | Meaning |
| --- | --- | --- |
| ◆ | waiting | An approval card or a question of the run waits for you. |
| ■ | blocked | The last status of the run starts with `blocked:`. |
| ▲ | stuck | The run is idle with unread mail, or its screen did not change for `[detach] stuck_after`. |
| ● | working | The run works. |
| ○ | idle | The run is at its prompt, and nothing is new. |
| ◇ | open | The ticket has a `Status:` line and no run. |
| ✓ | done | The last status of the run starts with `done:`. |
| · | ended | The run ended without a `done:` status. |

A ticket takes the most urgent state of its runs. An ended run stays on the board for 7 days when it has a ticket, and for 1 hour without one.
