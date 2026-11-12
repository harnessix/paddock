## The loft

Agents leave internal process files in repositories: instruction files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.cursor/rules/` and others), design and plan files (`DESIGN.md`, `PLAN.md`, `NOTES.md`, `TODO.md`), Spec Kit's `.specify/` and `specs/`, and new files under `docs/`. A push publishes them. The loft is a private layer above the project for these files. Each repository gets one loft in `~/.local/share/paddock/loft/<repository key>/`. The loft is a git repository of its own, so you can see the history of the agents' notes, specs and instruction files, and undo them, apart from the project's history.

| Command | What it does |
| :--- | :--- |
| `paddock loft init` | makes the loft of the current repository and turns it on. `.paddock` in the project becomes a link to the loft's notes |
| `paddock loft status` | the state, the private files that still sit in the project, and the changes since the last commit |
| `paddock loft log`, `paddock loft diff` | the history of the loft, and the changes since its last commit |
| `paddock loft commit` | commits the changes now. In sync mode, it syncs this work tree first |
| `paddock loft sync` | syncs the real `.paddock` folder of this work tree with the loft now. `--watch` keeps syncing until Ctrl-C |
| `paddock loft trash` | lists the loft trash: the notes that a sync deleted, with the time of each delete |
| `paddock loft restore PATH` | brings a note back from the loft trash to the loft and to the work trees. A folder name such as `notes/` brings back each note below it |
| `paddock loft mode` | shows the mode. `paddock loft mode sync` or `paddock loft mode link` sets `[loft] mode` in the user configuration and switches this work tree |
| `paddock loft adopt` | moves the private files of the repository into the loft, after a question. It says how to drop them from git history |
| `paddock loft remove` | takes the `.paddock` link away and turns the loft off. The files and the history stay |

With the loft on, every sandbox of the repository (also each `--worktree` sandbox and each spawned worker) sees the private files of the loft in place, for example `CLAUDE.md` at the project root. `.git/info/exclude` keeps them out of git. paddock commits the loft at the end of each run, one commit per run, with the sandbox id and the harness in the message. A private file that is still in the project stays there until you run `paddock loft adopt`.

Every sandbox also gets the notes folder `.paddock/` for the agents' own design notes and plans, with or without `paddock loft init`. It is in the loft, the same for the whole spawn tree, and outside git. The host request inbox stays at `.paddock/inbox/`. Outside a sandbox, `.paddock` in the project is a link to the loft's notes after `paddock loft init`, so the agents and tools that you run on the host see the same notes.

### The sync mode

In link mode, the default, `.paddock` in the project is a symlink to `~/.local/share/paddock/loft/<key>/tree/.paddock`. Some agent tools, for example Claude Code, treat the target of the link as a place outside the project, so every write to a note asks you for approval. With `[loft] mode = "sync"`, `.paddock` is a real folder of the project, and tools treat it like any project folder.

- Each work tree of the repository has its own copy. paddock syncs each copy with the loft in both directions: at the start and the end of each run, during the run when either side changes, and on `paddock loft sync` and `paddock loft commit`.
- paddock adds `/.paddock` to `.git/info/exclude`, never to `.gitignore`. Git never sees the folder, in every work tree.
- If both sides changed a file since the last sync, paddock keeps both. The loft version stays at the name. The other version goes next to it as `NAME.conflict-<host or run>-<time>`, on both sides. `paddock loft status` lists the open conflicts. To close a conflict, keep what you want in the file and delete the copy.
- A delete reaches the other side only if the other side did not change the file since the last sync. Otherwise the file stays, and the sync reports the delete.
- A delete never removes a note from the loft. When a sync finds a file missing in a work tree, it moves the loft version to the loft trash, `~/.local/share/paddock/loft/<key>/trash/<time>/<path>`. The other work trees then remove the file. `paddock loft trash` lists the trash, and `paddock loft restore PATH` brings the newest version of PATH back to the loft and to the work trees. If the loft has a newer file at PATH, the restored file becomes a conflict copy `PATH.conflict-<host>-<time>` next to it. Restore again to get an older version of the same path.
- `paddock gc --yes` removes the folders of the trash that are older than 30 days. `[loft] trash_days` changes the number. Nothing else removes them.
- The move to the trash does not happen at once. The sync that finds the file missing keeps it in the loft and reports that the delete waits. A later sync, at least 5 seconds after the first one, moves it to the trash if the file is still missing and the folder is the same folder, with the same marker. During a run, the watcher does that second sync by itself, also when the run ends. After `paddock loft sync`, run it again 5 seconds later, or let the next run do it. A jump of the system clock does not shorten the wait.
- A sync that finds a delete writes nothing new into the folder: no file from the loft and no new marker. The next sync does that, and the watcher starts it at once. So a new file cannot keep a folder alive that `rm -rf` is removing.
- If most files of the folder are gone at once (4 files or more), paddock moves none of them to the trash, reports it, and the next sync brings them back. A file with the same content at a new path counts as a move, not as a delete, so a `mv` of a folder works. To delete many notes, delete them in the loft, or delete a few at a time.
- Each write goes to a new file first and then replaces the old file in one step. If a sync stops halfway, no half file stays, and the next sync finishes the work.
- A folder that paddock did not sync before starts fresh: its first sync only adds files and never deletes one. This is also true after `git clean -fdx` or `rm -rf .paddock`, for a worktree that comes back at the same path, and for a restored backup of the folder.
- If `rm -rf .paddock` or `git clean -fdx` runs during a sync, the sync can miss some files. It does not delete them in the loft: their deletes wait for a second sync. That sync finds the folder gone or made again, so the deletes do not happen, and the sync reports it. A slow rm that takes longer than the waits moves some notes to the trash. `paddock loft restore` brings them back, for example `paddock loft restore notes/` for a folder. If the folder or its marker changes during a sync, that sync only copies new files to the loft and writes nothing to the folder. The next sync starts fresh. paddock knows its folder by the file `.paddock/.loft-sync` and by the inode of the folder. Do not copy that file to another folder.
- A program that keeps a note open and appends to it (a log, for example) can lose the bytes that it writes while a sync replaces that note. A tool that saves the whole file again loses nothing.
- During a run, the sync writes only to the log. When the run ends, paddock prints a summary of the conflicts that the sync found during the run.
- paddock does not follow symlinks in the folder. It skips them and reports them. `inbox/` stays with each work tree.
- `paddock loft mode sync` turns the link into a real folder with the files of the loft. `paddock loft mode link` syncs first, and then makes sure that each file in the folder is in the loft with the same content. If a file is not, the folder stays and nothing changes. Each file is checked again just before paddock removes it. A file that changed in the meantime stays in `.paddock.to-link-<time>`. While another run of the work tree is active, paddock refuses the switch to link.

A project `paddock.toml` can set `[loft] mode`, and nothing else of `[loft]`. The project value wins over the user configuration. Sandboxes see the real folder like any project folder. Nothing in `.paddock` is ever pushed or committed to the project.

```toml
[loft]
store = "external"     # default. "repo": the private paths stay in the repository
auto = true            # a loft for every project at its first paddock run. Default: false
paths = ["notes/"]     # added to the built-in list. A folder ends with /
public = ["AGENTS.md"] # kept in git on purpose, with no notice
commit = "run"         # default. "manual": only `paddock loft commit` commits
mode = "link"          # default. "sync": .paddock is a real folder that paddock syncs
trash_days = 30        # default. paddock gc removes deleted notes older than this from the loft trash
```

A project `paddock.toml` can only set `[loft] mode`. At the start of a run, paddock names the private files that git tracks or can add, unless you marked them public. The push approval has one line for each kind of private file in the push (instruction files, design files, Spec Kit files, your own paths), and one line with the new files under `docs/` that the push adds, at most five names and then a count. These lines never block a push. You decide in the approval. `paddock explain` lists each private path, where it lives, and whether git tracks it.
