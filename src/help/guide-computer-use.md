## Computer use in the macOS guest

paddock runs each desktop app of app mode in its own macOS virtual machine, the guest (see `paddock help app-mode`). An agent can see and operate that app with the `paddock mac` commands. The commands work in the guest only. They never move your pointer, never type on your Mac and never read your clipboard. You keep your Mac while the agent works in the app.

### Start

1. Build the golden image once with `paddock macvm setup`. The setup allows Accessibility and Screen Recording for the guest agent, so nobody grants a permission by hand. A golden image from an older paddock has no Screen Recording permission: run `paddock macvm setup` again.
2. Start the app: `paddock app run notes --backend macvm`.
3. Give the agent the commands, or the MCP server:

```sh
paddock mac status --app notes
paddock mac screenshot --app notes --out notes.png
paddock mac click 412 230 --app notes
paddock mac type 'Hello' --app notes
paddock mac key cmd+s --app notes
paddock mac ax --app notes
```

Every command prints one JSON object. A failure prints `"ok": false`, a `code` and an `error`, writes the reason to stderr, and exits with 1.

### The target

- `--app NAME` works on the windows of that app session only: its windows, sheets, menus and panels. A screenshot shows only those windows, cropped to them. Input outside them is refused, and input on a point that another window covers is refused.
- `--app NAME --desktop` works on the whole guest display of that session.
- `--instance N` selects another instance of the same app.
- Without `--app`, a command works on the only running session, on its whole display. If more than one session runs, the command names them and asks for `--app`.

### Coordinates

Coordinates are pixels of the last screenshot of the same target. paddock keeps the size and the place of that screenshot for each session and target, and for each client: the command line, each `paddock mcp mac` and each sandbox have their own. So a screenshot that you take at another size does not move the clicks of an agent. If the app's windows move, the coordinates move with them. If the windows change size, an action fails with `stale` and asks for a new screenshot. By default a screenshot fits the image limit of a model (1568 pixels on the long side, 1.15 megapixels). `--max-size full` gives the native pixels, and `--max-size 1280x800` gives another limit.

### The commands

| Command | What it does |
| :--- | :--- |
| `status` | The display, the permissions, the screen lock, the app and its windows. |
| `screenshot [--out FILE] [--max-size SIZE] [--cursor]` | A PNG of the target. |
| `click X Y [--button right] [--double] [--mods cmd,shift]` | A click at a pixel. |
| `move X Y` | The pointer goes to a pixel, with no click. |
| `drag X Y TO_X TO_Y` | Press, move, let go. |
| `scroll X Y up\|down\|left\|right [LINES]` | Scroll at a pixel, 3 lines by default. |
| `type TEXT` or `type --stdin` | Text as it is, on any keyboard layout of the guest. |
| `key COMBO...` | Keys one after the other: `cmd+s`, `shift+tab`, `return`. |
| `ax [--all]` | The accessibility elements: role, label, value, frame, center and id. |
| `press ID [--role ROLE]` | The press action of an element from `ax`. |

In app scope, the app comes to the front before keys and text go in. Before each key combination and each part of the text, the guest agent checks that the app is still the front app. If another app came to the front, the rest is not sent, and the command fails with `not_front`. In app scope, the known shortcuts of the system are refused, by any name of their key (`s_pace` is `space`): `cmd+space`, `cmd+tab`, Force Quit, lock and log out, the screenshot tools, Mission Control and the Spaces (`ctrl` with an arrow or a digit), the Dock, the Help menu (`cmd+shift+/`), `fn` alone or with a letter, and the function keys with a modifier (`ctrl+f2` is the menu bar). Use `--desktop` for them. In app scope, a click on the menu bar (the top 24 points of the display) is refused. While a menu is open, only `escape` goes: every other key, text and click fails with `covered`. If a key combination opens a menu, the rest of the request is not sent. A menu that a key of the app opens is seen only after the guest shows it, so the next key of the same request can reach it. From a menu, the arrow keys reach the Apple menu and Services. The list is of the known shortcuts only: a shortcut that the user changed or added, or one of a service, is not refused. No input goes to a locked guest screen.

### MCP

`paddock mcp mac [--app NAME]` is an MCP server on stdio. Each tool has the name of a command with `mac_` in front, for example `mac_click`. The tool for `ax` is `mac_elements`. A screenshot comes back as an image. Each action takes `screenshot_after: true` to return a new screenshot after the action. If that screenshot fails, the action still ran: the result has `ok: true` and a `screenshot_error` with the code. Each tool takes `app`, `instance` and `desktop` to select another target.

### From a sandbox

In a sandbox, `paddock-host mac COMMAND` and `paddock-host mcp mac` have the same commands and tools. Each call is a request of the capability `mac` to the host. It is `ask` by default: the first request shows a card, and "allow for this session" covers later requests on the same app session and scope. paddock chooses the session before it asks: the card names the session and the pid of its helper, and the grant ends with that session. If the session stops and starts again, or another session runs instead, paddock asks again. A grant for the windows of an app does not cover the whole guest display. The card says what a grant reaches: in app scope, clicks on the app's windows and keys to the app while it is in front; with `--desktop`, every app of the guest and the host folders that are shared with the session. While a window of the session is key on your Mac, your clipboard is copied into the guest, so a paste in the guest can read it. The card shows every character of the text that the agent wants to type, a Return as ⏎ and a Tab as ⇥. A sandbox types at most 1000 characters in one request; a longer text is refused and goes in parts. The card of `press` without `--role` says that a changed window can make it press another element. Only the newest 20 screenshots of the broker (`paddock-mac-SECONDS.png`) stay in `.paddock/inbox/`; paddock never removes another file there. The audit log keeps the tool, the target and the points, never the typed text. A screenshot goes to `.paddock/inbox/` of the project. Set `mac = "deny"` in `[host]` of the user configuration to turn it off.

### The skill

`paddock skill install` installs the skill `paddock-mac`. It teaches an agent the loop: observe with a screenshot and the elements, do one action, then verify with a new screenshot.

### How it works

The guest agent listens on the vsock port `guest-computer` (1034). Only the host can connect, through the control socket of the session. The agent captures with ScreenCaptureKit and posts input with CGEvent into the guest session. The accessibility snapshot reads the tree of each window of the target, 12 levels deep by default, at most 1500 elements, within 12 seconds.

The same port carries a frame stream for a live view. `frames.subscribe` takes a target and `max_fps`. Then the agent sends a PNG with a sequence number and its size each time that the picture changes. `frames.stop` ends it.

### Limits

- These parts have no test on a Mac yet: the capture, the input, the accessibility snapshot and the frame stream in a guest. The Screen Recording permission of a new golden image also has no test on a Mac yet.
- Key names are positions of a US keyboard. A shortcut with a letter also carries the letter. An app that reads only the key position can see another letter on another layout. Text is never affected.
- The capture reminder of macOS for Screen Recording can come back in a later macOS version. If a screenshot fails with `screen_recording` on a new golden image, run `paddock macvm status`.
