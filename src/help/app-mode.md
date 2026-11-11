## App mode

Desktop agent apps run in a sandbox too: ChatGPT with Codex, Claude Desktop, VS Code, and flatpak apps. They get the same file, git, secrets, environment and network rules as a terminal harness, and their windows are normal windows on your desktop. Flatpak apps have some limits, described at the end of this section.

<p align="center">
  <img src="docs/assets/app-demo.gif" alt="A recording: a host terminal shows an SSH key and a .env with a token, then runs paddock app run vscode; in VS Code's own terminal, the SSH key does not exist, .env is 0 bytes, and an edit to the project works" width="100%">
</p>

```sh
paddock app list                          # the apps paddock knows, and their launcher entries
paddock app run claude-desktop ~/src/api  # run an app with one or more workspace folders
paddock app install chatgpt-desktop ~/src/web   # add a launcher entry that runs it in paddock
paddock app uninstall chatgpt-desktop
```

The app sees only the folders that you give it, from the command line or from `[apps.NAME] roots` in `~/.config/paddock/config.toml`. Its windows connect to your compositor through a restricted socket: the app cannot capture the screen, read the clipboard in the background, type into other windows or list them. Flatpak apps use flatpak's own Wayland socket instead. Add `--gpu` for hardware rendering.

The app gets a session bus of its own, inside the sandbox. It cannot reach your session bus, your keyring or the system services. Three services run on its bus:

- A private keyring (gnome-keyring). The app stores its login and other secrets there, encrypted with a password that paddock keeps on the host. Claude, ChatGPT and VS Code use it. You log in once inside the sandbox, and the login stays for later runs.
- Links. When the app opens a web link, paddock asks you, the same way as for `paddock-host open`, and then opens the link in your browser. The app's request for a link through the desktop portal works the same way.
- Notifications. They appear on your desktop, with the app's name.

The app gets no file chooser portal. It uses its own file dialog, which shows only the files in the sandbox. If an app needs the real desktop portals, give it the filtered host bus in `~/.config/paddock/config.toml`:

```toml
[apps.NAME]
bus = "host-filtered"
```

On that bus the app reaches only the desktop portals, notifications and the tray. It gets no keyring, so it keeps its login in a plain file in the sandbox home. Flatpak apps always use flatpak's own filtered host bus.

### One worktree per app

Two agents in one checkout switch branches and overwrite each other's changes. With `--worktree`, the app gets its own git worktree of the first workspace folder, on a new branch `paddock/<app>-<date>-<id>`. paddock creates it in `~/.local/share/paddock/worktrees`, the same way as `paddock <harness> --worktree`. Your checkout keeps its branch and its changes.

```sh
paddock app run vscode --worktree ~/src/api
```

To use a worktree on every start, set `worktree = true` in `[apps.NAME]` of `~/.config/paddock/config.toml`. When you start an app from the menu without saved folders, the folder dialog asks whether to open a git repository in a new worktree. "New worktree" is the default button.

When the app exits, paddock prints the commits and the uncommitted changes in the worktree, and the commands to review, merge or remove it. If no terminal shows that text, for example after a start from the menu, a notification gives the branch and the number of commits and changes, with a Merge and a Remove button where the notification server supports buttons. If the app made no commit and left no change, paddock removes the worktree and the branch at once. A worktree with work stays until you remove it, or until its work is merged (see Automatic cleanup (`paddock help storage`)).

Flatpak apps use worktrees too. paddock gives the app the worktree folder and the repository's `.git` folder read-write, so git in the app can commit there. The `.git` file of the worktree, the `config`, `config.worktree`, `commondir`, `gitdir` and `hooks` of the repository and of the worktree stay read-only.

Later, the commands and the "Worktrees" row of the bar panel do the same:

```sh
paddock worktree list                       # size, last use, branch, merged state, uncommitted changes, in use
paddock worktree merge paddock/vscode-20261102-1a2b
paddock worktree remove ~/.local/share/paddock/worktrees/api/wt-api-1a2b3c [--force]
```

- `list` shows each worktree that paddock made, for apps and for `--worktree` runs. `--json` gives data.
- `merge` merges the branch into the current branch of the repository. It uses paddock's hardened host git, so no hook and no program from the repository configuration runs. It refuses when the repository has uncommitted changes. On a conflict it aborts the merge, and the repository stays as it was. Uncommitted changes in the worktree are not part of the merge.
- `remove` removes the worktree and deletes its branch. It refuses while a sandbox works in it. Without `--force`, it refuses when the branch has commits that no other branch has, or when the worktree has uncommitted changes.
- To keep a worktree, do nothing.

The panel row lists the worktrees with Merge, Remove (press twice) and Open folder buttons.

A flatpak app becomes an app profile with `[app] flatpak = "ID"`. paddock drops every permission that the app asks for, also those from `flatpak override`, and adds back only the workspace folders, the Wayland socket, the portals and, with `--gpu`, the GPU. Many flatpak apps ask for your whole home directory and the host bus, so their own sandbox is not enough. The network is `egress` or `none`, and the host passthrough (`paddock-host`) is not available.

The workspace folders of a flatpak app get the same protection as for other apps. paddock refuses the home directory and protected paths such as `~/.ssh`. In each folder, `.git` is a mount point of its own, its `config`, `config.worktree`, `commondir`, `gitdir`, `objects/info/alternates` and `hooks` are read-only, and the folders in `secrets.hide` are hidden. The host environment passes through the same filter. flatpak can hide a folder, but not a single file. So when the secrets detector finds a file such as `.env` or `id_ed25519` in a folder, paddock does not start the app, and names the files. Move them out, or put them in a folder and add that folder to `[secrets] hide`. To start the app anyway, with these files visible to it:

```toml
[apps.NAME]
unprotected_workspaces = true
```

This works only in `~/.config/paddock/config.toml`, never in a project's `paddock.toml`. paddock then prints a warning with the files on every start, `paddock app list` shows the setting, and the bar panel marks the running app.

Windows of app sandboxes get a colored border, so you can tell a protected app at a glance. "Protected windows" below describes the border. App sandboxes appear in the bar panel with the other sandboxes. Enter on one focuses its window.

### Protected windows

On Omarchy (Hyprland), each window that shows a sandbox gets a border, always: an app sandbox window, the display of window mode, and the terminal window of a terminal run. A terminal window counts when its terminal emulator holds the terminal of the run. Some terminal emulators draw several windows from one process, for example Ghostty, `foot --server` and kitty with a single instance. paddock cannot tell which of those windows holds the run, so none of them gets a border. An app window counts only when its process runs in the sandbox. A run in tmux, a detached run and a spawned child have no window of their own and get no border. On macOS the rule for terminal windows is different: see App mode on macOS (`paddock help macos`).

- Sage green: the sandbox reaches the network only through paddock's proxy, or not at all (network modes `none`, `api` and `egress`).
- Amber: the sandbox has a wider network (`host`, and `allowlist` on srt).

paddock sets the border on each window at run time, and your Hyprland configuration does not change. After a run ends, its terminal window gets its own border back. On a light Omarchy theme, both colors take a deeper tone.

When a protected window gets focus, a small badge shows on its top edge for a few seconds, for example `paddock · dev · egress · 2h`: the pasture or the harness, the network mode, and the run time. The badge stays while the pointer is on it. A click opens that sandbox in the bar panel. The badge never takes the keyboard, and only the badge itself takes the pointer.

The bar widget shows a shield with the number of protected windows. The "Protected windows" row of the panel lists them, and a click on one focuses it. Bind a key to `omarchy-shell harnessix.paddock windows` to open the list directly.

```sh
paddock windows                  # the protected windows
paddock windows --json           # with level, badge text and geometry
paddock windows --focus 0x55d0   # focus one protected window
```

Known limits:

- Desktop apps log in again inside the sandbox, because your keyring is out of reach. The login then stays in the private keyring of the sandbox. An app with `bus = "host-filtered"` has no keyring. Claude, for example, then warns that no system keyring is available and stores its secrets without encryption, in the sandbox home, which only your user can read.
- The app cannot open files with your desktop apps or pick files outside the sandbox. Its file dialog shows only the sandbox.
- A new window that gets focus can read the clipboard once. This is how Wayland clipboards work, and Flatpak has the same gap.
- Tray icons may not appear.
- Apps cannot update themselves inside the sandbox. Update them on the host, with your package manager or the app's own installer outside paddock.
- To add a folder, restart the app. A running sandbox cannot get new mounts.
- Custom links (`claude://`, `codex://`, `vscode://`) reach the app in its sandbox after `paddock app install`, which makes paddock their handler. `paddock app uninstall` gives them back to the old handler. A login page that the app opens through paddock and that returns to `http://127.0.0.1:PORT` reaches the app: paddock opens that one port on 127.0.0.1 of your machine for ten minutes, while the app listens on it. If an app opens its login page in another way, set `[apps.NAME] login_ports = true` in `~/.config/paddock/config.toml`. Then all the ports the app listens on are open on 127.0.0.1 of your machine while it listens, for any local program.
