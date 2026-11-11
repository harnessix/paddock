## Limits

The project directory is writable, and you use its files on the host later. Some host tools run project files without being asked: direnv runs `.envrc`, mise runs hooks in `mise.toml`, editors run tasks. paddock reports changes to these files when the sandbox exits. Review them before you use them on the host.

The agent can read its own API token and the variables that it inherits. With network access, it can send them elsewhere. Use `--env clean` and an isolated login to limit what it holds.

pasta hides the host addresses that exist when the sandbox starts. If the host gets a new address while a sandbox runs, for example from a VPN that starts later, services on it are reachable from that sandbox until the next start. paddock watches for new addresses, shows a warning in the bar panel, and prints it at exit.

By default, uv and npm share writable caches within a repository or plain project folder. Worktrees, spawn children and trainers use the same project caches; other projects cannot see them. These tools support concurrent cache writers. Other tools keep separate worktree upper layers. Peer caches use the peer and repository identity, so worktrees share a peer cache. When an eligible old worktree cache is idle and the project cache is empty, paddock moves it into the project cache without copying it.

Choose the sharing boundary in the user configuration (`~/.config/paddock/config.toml`):

```toml
[cache]
share = "project" # default; also "verified" or "none"
```

`verified` gives each run its own writable cache and uses verified read-only lower layers where available. `none` uses only per-run caches. Both choices isolate writable peer caches too. Project files cannot set this option. `paddock explain` shows the choice and layer. `paddock pasture list` names active `lease:cache:TOOL` locks and their runs. GC and limits keep a cache while its lock is held. The host `XDG_CACHE_HOME` is not inherited by default; an explicit `[env] pass` can still pass it.

The srt and Seatbelt backends cannot remap these cache mounts. Their caches stay in the sandbox home, and `[cache] share` does not apply to them. Seatbelt can also refuse a run that requests an overlay. `paddock explain` reports these limitations for the selected backend.

The shared mise installs are read-only in every project sandbox, and no project file runs while paddock writes them (see mise toolchains). On the bwrap backend, other package caches use overlays. The lower layer under `$XDG_CACHE_HOME/paddock/layers` is shared by all projects and is read-only in every sandbox. Only paddock writes it on the host. The upper layer under `$XDG_CACHE_HOME/paddock/projects` belongs to one worktree and takes its writes. A file that one project changes in its cache never reaches another project.

After a bwrap sandbox with worktree overlays ends, paddock checks eligible downloads in the upper layer in the background, for at most two minutes. It copies a package into the shared layer only when its content matches a source that no project can change:

- cargo: a `.crate` file from crates.io, against the `cksum` in the crates.io sparse index, fetched on the host.
- npm: existing worktree upper-layer files of `_cacache/content-v2`, against the sha512 or sha256 hash in their own paths.
- go: a module zip and its `go.mod`, against the `h1:` hashes of the checksum database `sum.golang.org`, fetched on the host.

A file that fails the check, or that paddock cannot check (no network, another registry), stays in the project. paddock never replaces a file that is already in the shared layer. The audit log records each result with the cache, the package and the version, never the content. A small record in the paddock state keeps each checked package version, so paddock asks the index only once for it. pip, pnpm, bun and yarn have no shared lower layer. uv and npm normally use the separate project caches described above. Unpacked sources and build caches always stay in each project.

The docker and podman backends have no overlay. They bind each worktree's folder read-write. `[storage] layered_caches = false` does the same on the bwrap backend. uv and npm use their project caches on these backends too. No setting gives all projects one writable cache.

Some profile entries come from documentation, not from a test on a real install. `paddock doctor` lists them for each installed harness.

On rootful docker, an escape from the container is an escape to root. Use the bwrap backend or rootless podman when you can.
