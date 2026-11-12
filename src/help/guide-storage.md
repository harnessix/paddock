## Disk space

Each harness gets its own home in each project, so the homes grow with the number of projects. The docker and podman backends also keep one image for each configuration.

`paddock du` lists what paddock stores and how much space it uses:

- Each sandbox home: harness, login, project, size, last use, and what you lose when you delete it.
- The paddock images of docker and podman, and whether each is current or in use.
- The worktrees that `--worktree` created, with their total size, last use, unmerged commits and merged state.
- The build caches of worktree runs.
- The shared project caches and the package cache layers of each worktree.
- The session records of `paddock files`, and the keyring passwords of app homes.
- The project snapshots and the undo trash of `paddock undo`.
- The caches in the homes, by class (see Caches (`paddock help storage`)).

On btrfs, a size counts only the data that no other file shares. On other file systems, it counts the allocated blocks, as `du` does. The first line names the kind. `paddock du --json` prints the same data as JSON.

`paddock gc` removes what is no longer needed. Without `--yes`, it removes nothing and prints the list.

```sh
paddock gc                                  # what --yes would remove
paddock gc --yes                            # remove the automatic items
paddock gc --homes --older-than 30          # also list homes unused for 30 days
paddock gc --homes --yes                    # remove them
```

`--yes` alone removes these items:

- Paddock images that are not current and that no container uses. Images share layers, so `gc` counts only the part that belongs to one image alone.
- paddock's own records in the docker build cache. paddock marks its build steps, so the build cache of your other projects stays. A current image keeps its own layers, so this only makes the next image build slower.
- Keyring passwords whose home is gone.
- Session records that ended more than 30 days ago.
- Homes of harnesses whose profile no longer exists.
- Old shared caches that paddock no longer uses, such as `~/.cache/paddock/shared/mise-installs` and the old writable package caches in `~/.cache/paddock/shared`. paddock keeps them while a sandbox runs.
- The package caches of gone projects and gone or merged paddock worktrees. Caches in use by a run or held by a live lease are kept; unrelated runs do not block their removal. If a shared uv or npm cache is empty, paddock moves an idle old worktree cache into it.
- Project snapshots whose project is gone.
- Pure caches in the homes of sandboxes that do not run (see Caches (`paddock help storage`)).
- The folders of the loft trash that are older than `[loft] trash_days` (default 30). They hold the notes that a sync of a `.paddock` folder in sync mode deleted (see The loft (`paddock help private-notes`)).

paddock also does this after it builds a new image. To turn that off, set `PADDOCK_GC_AFTER_BUILD=0`.

The other items need a flag that names them:

- `--homes` adds homes whose project is gone, or that were not used for `--older-than` days (default 90). A home holds the transcripts and settings of a harness in one project. A terminal harness keeps its login in your home or in `~/.local/share/paddock/auth`, so the login stays.
- A desktop app keeps its login in its home. If you delete the home, you must log in again. These homes also need `--include-app-logins`.
- `--worktrees` adds paddock worktrees. A worktree with unmerged commits or uncommitted changes also needs `--force`. paddock then keeps its branch, so the commits stay in the repository.
- `--snapshots` adds project snapshots and undo trash folders older than `--older-than` days. The files that an undo moved to the trash are lost.
- `--scratch` adds scratch projects that were not used for `--older-than` days. An agent from `omarchy-agent` works in one when it starts in the home folder (see Only the agents that Omarchy starts (`paddock help defaults`)). Its files are lost.

paddock never removes the home or worktree of a running sandbox. It deletes only inside its own directories. It never follows a symlink, so a link in a sandbox home cannot point it at your files. A sandbox can edit the `.paddock-origin` file in its home, which records the project. paddock uses that file only when it matches the folder name of the home, which contains a hash of the project path.

### Automatic cleanup

paddock cleans up by itself, so its folders do not fill the disk:

- When a `--worktree` run or a spawned worker ends without a commit and without uncommitted changes, paddock removes its worktree and its branch at once. There is nothing to review.
- Each `paddock run` starts a cleanup in the background, at most once an hour. The start does not wait for it. It removes the worktrees whose work is merged and that have no uncommitted changes, and their `paddock/` branches. If a local branch outside `paddock/` has a commit, the commit counts as merged. A commit with the same patch counts too, for example after a cherry-pick or a rebase.
- A worktree with unmerged work stays. When it is not used for `worktree_cache_days`, paddock deletes its build output: folders with a `CACHEDIR.TAG` file, such as cargo's `target/`, that git ignores and that hold no tracked file. paddock never deletes a tracked file, or an untracked file that git does not ignore.
- The same cleanup removes snapshots and undo trash folders older than `snapshot_days`.
- It never removes a sandbox home. A home holds your conversations with the harness (transcripts and history) and an app's login. Only `paddock gc --homes` removes homes, when you ask, and it names what is lost.
- It also does what `paddock gc --yes` does, except for images and the loft trash.

The cleanup never touches the home, worktree or snapshot of a running sandbox. `paddock doctor` shows when it last ran, what it removed, and its errors. `paddock gc-auto --now` runs it at once.

### Free space

Before a sandbox starts, paddock checks the free space on the disks of the project, the worktrees, the homes and the build cache:

- Below the hard floor, paddock first runs the cleanup, for at most two minutes. If the space is still below the floor, the sandbox does not start. The message gives the numbers and the commands that free more space.
- Below the soft floor, the sandbox starts with one warning line, and a desktop notification at most once an hour.
- While sandboxes run, paddock checks every five minutes. At the hard floor it sends a notification. It does not stop a running sandbox.

Each floor is the smaller of its two numbers, so it fits both small and large disks. The defaults are 15 GB or 5% for the hard floor, and 50 GB or 10% for the soft floor. `paddock doctor` shows the free space and the floors.

### Build cache of worktree runs

A `--worktree` run or a worker of a Rust project does not build into `target/` of its worktree. Its cargo target folder (`CARGO_TARGET_DIR`) is in `~/.cache/paddock/build/<repository>/`. At the start, paddock copies the newest target folder of the same repository with copy-on-write, so the worker does not compile the project from nothing. When the worker ends, its folder becomes the newest one. Each worker has its own copy. Two workers that build different commits do not wait for each other, and they do not see each other's output. A project never sees the build cache of another project.

A run started in a worktree that paddock made, without `--worktree`, uses the same build cache.

Builds in a sandbox write less to the disk by default (`[storage] lean_builds = true`). paddock sets `CARGO_INCREMENTAL=0`, so cargo does not rewrite its incremental folder on every build, and `CARGO_PROFILE_DEV_DEBUG` and `CARGO_PROFILE_TEST_DEBUG` to `line-tables-only`, so backtraces keep file and line numbers but the test binaries are much smaller. A value that you set in `[env]` stays. Set `lean_builds = false` to build as on the host.

On btrfs and xfs the copy takes almost no space. On ext4 and other file systems without copy-on-write, each worker starts with an empty folder, and paddock deletes it when the worker ends. On macOS the build cache is off. The cleanup keeps the build caches of all repositories under `build_cache_gib` and removes the least recently used first.

A script that reads `target/` directly does not find the build output. For such a project, set this in its `paddock.toml`:

```toml
[setup]
shared_build_cache = false
```

### Cleanup settings

Set these in `~/.config/paddock/config.toml`. The values here are the defaults. 0 turns a rule or a floor off.

```toml
[storage]
auto_gc = true               # the automatic cleanup; PADDOCK_AUTO_GC=0 also turns it off
worktree_cache_days = 2      # delete the build output of a kept worktree after this many days without use
snapshot_days = 30           # delete snapshots and undo trash folders older than this
hard_floor_gib = 15          # do not start a sandbox below the smaller of these two
hard_floor_percent = 5
soft_floor_gib = 50          # warn below the smaller of these two
soft_floor_percent = 10
build_cache = true           # the build cache of worktree runs
build_cache_gib = 40         # the size of all build caches together
package_cache_project_gib = 20  # package caches for one repository or folder
package_cache_total_gib = 100   # package caches for all projects
```

If a value is wrong, paddock uses the default, and the run and `paddock doctor` print a warning.

The package cache limits apply before and after a run. paddock removes the oldest whole tool caches first. It keeps caches that a run uses, so the limits can be exceeded until runs end. Set either package cache limit to 0 to turn off that limit. These limits apply even when `auto_gc` is off.

`paddock doctor` warns when a filesystem is nearly full. On Btrfs, it also reports unallocated space and metadata use. If it cannot read those values, it reports that they are unavailable.

### Caches

paddock sorts the caches in a sandbox home into three classes. `paddock du` shows the size of each class.

- Pure caches rebuild themselves: the mesa shader cache, the fontconfig cache, the `Cache`, `Code Cache` and `GPUCache` folders of Electron and Chromium apps under `~/.config/APP`, and `~/.codex/cache`. In the sandbox, each of them with a known path is an empty tmpfs, so it is gone at exit. The tmpfs uses memory while the sandbox runs. The Electron folders get their names from the app, so `paddock gc --yes` clears them instead. paddock never clears a login, settings or history folder.
- Package caches check each entry themselves: `~/.npm`, `~/.cache/pip` and `~/.cache/uv`. A project with a `package.json`, a `pyproject.toml` or a similar file gets one shared copy for all projects, in `~/.cache/paddock/shared`. A harness that installs through npm (Claude Code, codex, gemini, copilot) gets the shared `~/.npm` in every project.
- Runtimes, such as `.codex/packages` and `.copilot/pkg`, stay in the home. Deduplication shares their data.

The list of pure caches is in `src/storage/caches.rs`. A harness profile adds its own with `[state] scratch = ["PATH"]`, relative to the home. A path with `*` matches one folder name, and only `paddock gc` clears it. To turn off the tmpfs mounts and the gc step, set this in `~/.config/paddock/config.toml`:

```toml
[storage]
tmpfs_caches = false
```

### Deduplication

Harnesses download the same runtimes into the home of each project. For example, `.codex/packages` holds about 400 MB, and it is the same in each project. On btrfs, xfs and bcachefs, the kernel can let identical files share their data (`FIDEDUPERANGE`). The kernel compares the bytes first and refuses data that differs. A later write to one copy gets its own blocks, so the other copy does not change.

```sh
paddock gc --dedupe                         # count the files with the same data and their size
paddock gc --dedupe --yes                   # share the data and print the space that came back
```

After each sandbox ends, paddock also shares the data of the files in its home with the other homes. This runs in the background at the lowest priority, for at most two minutes, and the exit does not wait for it. To turn it off, set this in `~/.config/paddock/config.toml`:

```toml
[storage]
dedupe = false
```

paddock reads only files of at least 4 KiB in the sandbox homes. It groups them by size, then by SHA-256. It keeps the hashes in `~/.cache/paddock/dedupe/index`, so it reads each file once. It never follows a symlink, never leaves the file system of the homes, and skips a file that changes while it reads it. On ext4, tmpfs and other file systems that cannot share data, paddock skips this step, and `paddock du` says so.
