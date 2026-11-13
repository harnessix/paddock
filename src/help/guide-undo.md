## Undo a run

The project is writable in the sandbox, and host tools run some project files later, for example `.envrc`, `mise.toml` hooks and editor tasks. Before each start, paddock takes a snapshot of the project files, so one command undoes a bad run.

```sh
paddock undo                  # the last session of this project
paddock undo --list           # the snapshots: time, harness, size
paddock undo 1a2b3c --yes
```

`paddock undo` lists the files that the run added, changed and deleted, and asks before it restores. Changed and deleted files come back from the snapshot. Added files go to `~/.local/share/paddock/undo-trash/TIME/`, so you can undo an undo by hand. The command refuses while a sandbox of the project runs. A file that changed after the session ended (you or another agent edited it) gets a warning and stays as it is. When paddock did not record the end of the session (it was stopped, or the end walk took longer than 20 s), every changed file counts as such. `--force` restores it too. At exit, paddock names `paddock undo` when the run changed files.

The snapshot:

- lives in `~/.local/share/paddock/snapshots/PROJECT/SESSION/`. paddock keeps the last 10 for each project.
- leaves out `.git/`, which has its own history, and the folders that `.gitignore` ignores, such as `target` and `node_modules`. paddock asks its hardened host git for that list. Outside git, it leaves out `target`, `node_modules`, `.venv`, `dist` and `build`. `paddock undo` never touches these paths, also when a folder became ignored only during the session.
- shares the file data with the project on btrfs, xfs and bcachefs (`FICLONE`), so it takes almost no space. On other file systems, paddock copies the files only while the copy stays under `[snapshot] max`.
- has a time limit of 20 seconds. A slower snapshot is dropped with a notice, and the sandbox starts anyway.

A `--worktree` run has its own branch and gets no snapshot. Set the options in `~/.config/paddock/config.toml`:

```toml
[snapshot]
enabled = true     # the default
max = "500M"       # the largest full copy, without shared file data
```

## Work on a copy

With `--copy`, the agent works on a copy of the project, and your folder stays as it is until you apply. A copy is the default for a folder without git and for a run from a launcher entry. `--direct` works in the folder itself.

```sh
paddock run claude --copy     # the agent gets a copy of the project
paddock changes               # the runs that work on a copy
paddock changes 1a2b3c        # new, changed and deleted files, and conflicts
paddock changes 1a2b3c --diff # the changes as a diff
paddock apply 1a2b3c          # write the changes to your folder
paddock apply 1a2b3c src/     # only one folder or file
paddock keep 1a2b3c notes.txt # leave a file out of an apply of everything
paddock discard 1a2b3c a.txt  # drop a change from the copy
paddock undo                  # take the last apply back
paddock backups               # the backups of this project
paddock restore 4d5e6f        # bring the files of a backup back
```

paddock makes a backup at the start of each run on a copy, and before each apply and each restore. So every write into your folder has an undo. A file that you and the agent both changed is a conflict. paddock never overwrites it. Choose per file with `paddock apply 1a2b3c PATH --on-conflict original`, `copy`, `both` or `merge`. `both` puts the agent's version next to yours as `NAME.conflict-RUN-TIME`.

The copy:

- lives in `~/.local/share/paddock/copies/STORE/`. Your project holds only `.paddock/pointer.json` with the id of the store, so a copy of the project folder carries no backup and no note along. `paddock pointer status` shows the state, also after you moved or copied the folder.
- shares the file data with the project on btrfs, XFS, bcachefs and APFS, so it takes almost no space. On other file systems, it is a plain copy up to `[copy] max`.
- has the same rules as a snapshot: `.git/` and build output are never part of the change list. `.git/` comes along, so git works in the copy.
- is not encrypted by paddock. It relies on the encryption of your disk, and `paddock doctor` reports its state.

Set the options in `~/.config/paddock/config.toml`:

```toml
[copy]
default = "auto"   # auto: launcher runs and folders without git. always, never
apply = "ask"      # auto: apply at the end of a run with auto approvals and without a conflict
max = "1G"         # the largest plain copy, without shared file data
```
