## Launchers

`paddock launchers install` adds a launcher entry for every harness and app that paddock finds on this machine, with the paddock icon, in a "Paddock" folder:

- In the Omarchy menu (Super+Space), a Paddock submenu. paddock writes it as a marked block in `~/.config/omarchy/extensions/omarchy-menu.jsonc` and leaves the rest of the file as it is.
- In menus that follow the XDG menu standard (GNOME, KDE, Xfce), a Paddock folder.
- In any app launcher, search for "paddock".

A terminal harness entry opens a terminal with `paddock pick <harness>`: pick a recent project or type a path, and the harness starts there. An app entry runs `paddock app run`. If you did not save workspace folders for the app (`[apps.NAME] roots`), a dialog asks for one: pick a recent project or any other folder. The dialog needs zenity. Without it, a desktop notification says how to name the folders.

Logins work from these entries too. When a harness opens its login page, the approval card asks "Open the login page of claude in your browser?" and shows the link. After you allow it, the page opens in your browser on the host, and paddock forwards the one localhost port that the login returns to, for ten minutes and only while the sandbox listens on it. Login callbacks are on by default for claude, codex, copilot, gemini, opencode and agy (`[launch] login_callback` in the profile). They work on the bwrap, docker and podman backends; on srt, paddock prints a warning instead.

```sh
paddock launchers list      # what install would create
paddock launchers install
paddock launchers remove    # removes everything that install wrote, and nothing else
```

### Keeping the menu and the shims current

The menu entries, the Omarchy menu block and the shell shims list the harnesses that were installed when you turned them on. paddock keeps them current:

- On a harness start, paddock compares a cheap signal with the one of the last update: its version and the modification times of the folders on `PATH`. Installing an agent with mise or into `~/.local/bin` changes a folder on `PATH`. When the signal changed, paddock runs `paddock refresh --background` in the background, and the start does not wait.
- `paddock update` runs `paddock refresh` with the new version.
- `paddock refresh` adds the entries and shims of new harnesses and removes those of harnesses that are gone. The background run only adds: with a smaller `PATH`, it could miss a harness, and a missing shim would start that harness without the sandbox.
- Only the parts that you turned on change. `~/.bashrc` and `~/.zshrc` do not change.
- `paddock doctor` names the harnesses that the menu or the shims miss.
