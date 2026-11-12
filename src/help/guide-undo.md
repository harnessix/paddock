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
