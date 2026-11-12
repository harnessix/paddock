## Remove

1. Remove the bar widget:

   ```sh
   omarchy plugin remove harnessix.paddock
   ```

2. If you turned them on, remove the shell commands and the launchers. Do this before you remove the command:

   ```sh
   paddock shims remove
   paddock launchers remove
   ```

   `paddock setup --yes --no-shims --no-launchers --no-plugin` does steps 1 and 2 in one command.

   If you forget this step, nothing breaks. The line in `~/.bashrc` runs only when its script exists, and each command falls back to the real program when paddock is gone. The launcher entries stay in the menu until you delete them.

3. Remove the command. If you installed the Arch package (`bin/install` does this on Arch Linux), run `sudo pacman -R omarchy-paddock`. Otherwise, delete `~/.local/bin/paddock`.

4. Optional: to free space and keep paddock, run `paddock gc` instead. See Disk space (`paddock help storage`).

5. Optional: delete the data that paddock keeps. These directories hold the sandbox homes, isolated logins, shared caches, worktrees and settings. Your own logins and files are not in them.

   ```sh
   rm -rf ~/.local/share/paddock ~/.cache/paddock ~/.local/state/paddock ~/.config/paddock
   ```

   If you set `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` or `XDG_CONFIG_HOME`, the directories are under those paths instead.

6. Optional: if you used the docker or podman backend, run `paddock gc --yes` before you remove the command. It removes the old paddock images and paddock's build cache. To remove the current images too, run `docker image rm $(docker image ls -q paddock-base)`, and the same command with `podman`.

paddock does not change any file outside these directories, except the files that you name in `paddock env hide` (in `~/.config/paddock/config.toml`), the git worktrees that `--worktree` creates, the files that `paddock shims install` and `paddock launchers install` change (also through `paddock setup`), the `.paddock` link, the empty mount points and the `.git/info/exclude` lines of the loft (`paddock help private-notes`), and the paddock binary that `paddock update` replaces. Step 2 undoes the shell commands and launchers. To remove a worktree, use `paddock worktree remove`.
