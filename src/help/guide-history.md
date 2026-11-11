## Continue your host history in a sandbox (`--profile default`)

A normal sandbox has its own home, so the harness starts there without your conversations. With `--profile default`, the harness starts with its host data in the sandbox: conversations, sessions, settings, plugins and caches. You continue your real history.

```sh
paddock run claude --profile default
paddock run codex ~/src/app --profile default
```

The run works on a private snapshot. paddock copies the data roots of the harness into `~/.local/share/paddock/shadow/runs/ID/` and makes the sandbox home an overlay on that copy. Every change of the run goes into the overlay. When the sandbox ends, paddock prints how many files the run added, changed and deleted, with the first 20 paths, and removes the run folder. The host is never written.

The rules:

- The real home is never visible. Only the data roots of the harness come in. `~/.ssh`, your documents, other projects, the data of other harnesses and paddock's own folders are absent.
- paddock knows no vendor format. Whole files and folders come in as bytes.
- Logins belong to the credential sandbox. The credential files (`[state] auth`) are not part of the snapshot. They come in and sync back exactly as in every sandbox (`--auth`).
- `--profile default` is never the default mode. Only the command line turns it on. A project `paddock.toml` cannot turn it on or add data roots.
- With `--worktree`, the project of the run is the worktree. paddock does not tell the harness about the main project.
- bwrap runs it on Linux, Seatbelt on macOS. docker, podman, srt and the VM backend refuse.

The data roots are `[state] data` of the profile: `.claude` and `.claude.json` for claude, `.codex` for codex, `.grok` for grok. A harness without data roots refuses `--profile default`. Add roots in `~/.config/paddock/config.toml`:

```toml
[profiles.claude]
data = [".config/claude-extra"]

[storage]
shadow_copy_limit_mib = 2048   # the largest snapshot without reflinks
```

A data root is a path relative to the home. paddock refuses the home itself, a folder that many programs share (`.config`, `.local`, `.local/share`, `.local/state`, `.cache`, `.agents`), the key folders (`.ssh`, `.gnupg`, `.pki`, `.password-store`), its own folders, and paths with `..`.

The snapshot copies folders, regular files and the symlinks that stay inside their root. It skips symlinks that leave the root, sockets, FIFOs and devices, and names how many. On btrfs, xfs and bcachefs each file is a reflink (`FICLONE`), so the snapshot takes almost no space. On other file systems paddock refuses a snapshot above the size cap and prints its size. The run folder must be on the file system of the home.

paddock still binds the project, keeps the network mode, the secret masking, the host passthrough and the shared caches. Inside the data roots, the selective copies are off: no `seed` copies, no `json_import`, no history binds and no copy back. `import_ro` paths outside the roots, such as `~/.agents/skills`, stay. When the harness also runs on the host, paddock prints a notice: the snapshot is private, and nothing goes back. `paddock ps --json` marks such a sandbox with `"profile": "default"`.

On macOS, Seatbelt cannot mount an overlay. So the sandbox home is a writable copy of the data roots in `~/.local/share/paddock/shadow/runs/ID/home`, and the harness writes that copy. On APFS each file is a clone (`clonefileat`), so the copy takes almost no space and no time, and the start line says `(clones)`. On another file system, such as an external HFS+ disk, the start line says `plain copies` and the size cap applies. After its own writes, paddock records the size and the change time of each file. At the end it compares the copy with that record, prints the same summary and removes the run folder. A file that the run rewrote with the same size and the same change time is not in the summary. The other rules are the same as on Linux: nothing goes back to the host. The run uses the keychain of its sandbox login, not your login keychain. Claude Code keeps its login in the Keychain on macOS, so you log in once in the sandbox, and later runs with that login keep it.
