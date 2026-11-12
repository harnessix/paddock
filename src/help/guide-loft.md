## The loft

Agents leave internal process files in repositories: instruction files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.cursor/rules/` and others), design and plan files (`DESIGN.md`, `PLAN.md`, `NOTES.md`, `TODO.md`), Spec Kit's `.specify/` and `specs/`, and new files under `docs/`. A push publishes them. The loft is a private layer above the project for these files. Each repository gets one loft in `~/.local/share/paddock/loft/<repository key>/`. The loft is a git repository of its own, so you can see the history of the agents' notes, specs and instruction files, and undo them, apart from the project's history.

| Command | What it does |
| :--- | :--- |
| `paddock loft init` | makes the loft of the current repository and turns it on. `.paddock` in the project becomes a link to the loft's notes |
| `paddock loft status` | the state, the private files that still sit in the project, and the changes since the last commit |
| `paddock loft log`, `paddock loft diff` | the history of the loft, and the changes since its last commit |
| `paddock loft commit` | commits the changes now |
| `paddock loft adopt` | moves the private files of the repository into the loft, after a question. It says how to drop them from git history |
| `paddock loft remove` | takes the `.paddock` link away and turns the loft off. The files and the history stay |

With the loft on, every sandbox of the repository (also each `--worktree` sandbox and each spawned worker) sees the private files of the loft in place, for example `CLAUDE.md` at the project root. `.git/info/exclude` keeps them out of git. paddock commits the loft at the end of each run, one commit per run, with the sandbox id and the harness in the message. A private file that is still in the project stays there until you run `paddock loft adopt`.

Every sandbox also gets the notes folder `.paddock/` for the agents' own design notes and plans, with or without `paddock loft init`. It is in the loft, the same for the whole spawn tree, and outside git. The host request inbox stays at `.paddock/inbox/`. Outside a sandbox, `.paddock` in the project is a link to the loft's notes after `paddock loft init`, so the agents and tools that you run on the host see the same notes.

```toml
[loft]
store = "external"     # default. "repo": the private paths stay in the repository
auto = true            # a loft for every project at its first paddock run. Default: false
paths = ["notes/"]     # added to the built-in list. A folder ends with /
public = ["AGENTS.md"] # kept in git on purpose, with no notice
commit = "run"         # default. "manual": only `paddock loft commit` commits
```

A project `paddock.toml` cannot set `[loft]`. At the start of a run, paddock names the private files that git tracks or can add, unless you marked them public. The push approval has one line for each kind of private file in the push (instruction files, design files, Spec Kit files, your own paths), and one line with the new files under `docs/` that the push adds, at most five names and then a count. These lines never block a push. You decide in the approval. `paddock explain` lists each private path, where it lives, and whether git tracks it.
