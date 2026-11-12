---
name: paddock-mac
description: Use to see and operate a desktop app in paddock's macOS guest (computer use). Screenshots, clicks, text, keys, scrolls, drags and accessibility elements. On the host with paddock mac, in a sandbox with paddock-host mac, or with the MCP server paddock mcp mac. Never for the host desktop.
---

# Computer use in paddock's macOS guest

paddock runs each desktop app in its own macOS virtual machine, the guest. These tools see and operate the guest only. They never move the host pointer, never type on the host, and never read the host clipboard.

## Which command

| Where you are | Command |
| :--- | :--- |
| On the host | `paddock mac COMMAND` |
| In a paddock sandbox (`PADDOCK=1`) | `paddock-host mac COMMAND` |
| An MCP client on the host | `paddock mcp mac [--app NAME]` |
| An MCP client in a sandbox | `paddock-host mcp mac [--app NAME]` |

Each MCP tool has the name of a command with `mac_` in front, for example `mac_click`. The tool for `ax` is `mac_elements`. The tools take the same values as the commands below.

In a sandbox, the first request asks the user on the host. If the user selects "allow for this session", later requests on the same target do not ask again. A screenshot goes to `.paddock/inbox/`, and the output gives its path.

## The target

- `--app NAME` works on the windows of that app session only: its windows, sheets, menus and panels. A screenshot shows only those windows, cropped to them. Input outside them is refused.
- `--app NAME --desktop` works on the whole guest display of that session.
- `--instance N` selects another instance of the same app.
- Without `--app`, the command works on the only running session, on its whole display.

Use `--app NAME` unless the task needs another window of the guest.

## The loop: observe, act, verify

1. Observe. Run `status` first. It tells you whether the app runs, whether the screen is locked, and whether the guest agent has its permissions. Then take a `screenshot`. For controls, read `ax` (MCP: `mac_elements`): each element has a role, a label, a value, a frame and a center in pixels of the screenshot, and an id.
2. Act. Do one action: `click X Y`, `type TEXT`, `key cmd+s`, `scroll X Y down 3`, `drag X Y TO_X TO_Y`, or `press ID`. Give the coordinates as pixels of the last screenshot of the same target.
3. Verify. Take a new screenshot and make sure that the action did what you expected. Then do the next action. With MCP, `screenshot_after: true` on an action returns the new screenshot at once. If the result has `screenshot_error`, the action ran: do not repeat it, take a screenshot.

Do one action at a time. The screen can change after each action: a menu opens, a dialog comes, a window moves.

## Coordinates

Coordinates are pixels of the last screenshot of the same target, from the top left. The default screenshot size fits the image limit of a model, so the pixels that you see are the pixels that you give. If the app's windows move, the coordinates move with them. If the windows change size, an action fails with `stale`: take a new screenshot.

To click an element of `ax`, click its `center`, or press it with `press ID`. `press` does not move the pointer, and it works on a control that another window covers.

## Text and keys

- `type TEXT` types the text as it is, on any keyboard layout of the guest. A newline is the Return key. Use `--stdin` for a long text.
- `key COMBO...` presses keys one after the other: `cmd+s`, `shift+tab`, `return`, `escape`, `left`, `f5`, `cmd+shift+t`.
- In app scope, the app comes to the front before the keys go in. If another app comes to the front during the keys or the text, the rest is not sent (`not_front`).
- In app scope, the known shortcuts of the system (`cmd+space`, `cmd+tab`, Mission Control and the like) are refused, by any name of their key. A shortcut that the user changed or added is not known and is not refused. In app scope, the menu bar takes no clicks, and while a menu is open only `escape` goes. Use the app's own shortcuts and window controls, or ask the user for `--desktop`.
- From a sandbox, one `type` has at most 1000 characters. Type a longer text in parts.

## When a command fails

Every command prints JSON. A failure prints `"ok": false`, a `code` and an `error`, and exits with 1. Follow the reason. Do not send the same command again unchanged.

| Code | What to do |
| :--- | :--- |
| `covered` | Another window covers the point. Take a new screenshot, close or move that window, or use `--desktop`. |
| `outside` | The point is not on the screenshot or not on a window of the app. Take a new screenshot. |
| `stale` | The windows changed size. Take a new screenshot. |
| `not_front` | The app did not come to the front. Click a window of the app, then try again. |
| `locked` | The guest screen is locked. Tell the user. If the user uses a window of the session, paddock unlocks the screen. |
| `no_app`, `no_window` | The app does not run or has no window. Run `status`. If the app is gone, tell the user. |
| `screen_recording`, `accessibility` | The golden image has no permission for the guest agent. Tell the user to run `paddock macvm setup` again. |
| `denied` (sandbox) | The user did not allow the request. Ask in the conversation what the user wants instead. |

## Rules

- Do not type passwords or other secrets unless the user gave them to you for this task.
- An action can delete data, send a message, buy something or change an account. Before such an action, tell the user what you will do, and wait for an answer.
- The guest has its own firewall. A page that does not load can be a host that the user did not allow. Tell the user the host name.
