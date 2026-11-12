`paddock mac view` draws a macOS guest inline in your terminal. It shows the whole guest desktop, or only the windows of one app session. Your keys and your mouse go to the guest. If you leave the view, the session runs on.

### Start the view

```sh
paddock mac view --app notes              # only the windows of the app session "notes"
paddock mac view --app notes --instance 2 # instance 2 of that app
paddock mac view --app notes --desktop    # the whole desktop of that guest
paddock mac view                          # the desktop of the only running guest
paddock mac view --probe                  # what this terminal supports, then exit
```

The view needs a terminal that shows images. It asks the terminal at the start and uses the first protocol that works:

1. The kitty graphics protocol, for example in kitty, Ghostty and WezTerm.
2. iTerm2 inline images, in iTerm2 and WezTerm.
3. Sixel, for example in foot, xterm with sixel, and tmux 3.4 built with sixel.

To choose a protocol yourself, use `--protocol kitty`, `--protocol iterm2` or `--protocol sixel`. If no protocol works, no view starts: the command says why and exits with code 3, so a script can tell. The windows of an app session stay in their normal windows on the host desktop. `--probe` also exits with code 3 when no protocol works.

If the picture changes, the guest sends a frame. Otherwise it sends nothing. The view also skips a frame with the same pixels as the frame on the screen. A slow terminal gets the newest frame, not a queue of old frames. `--fps N` sets the most frames per second, 30 by default.

The picture fits the terminal and keeps its aspect. It is never larger than one guest pixel per terminal pixel, so a Retina frame scales down to the terminal. The view asks the guest for frames no larger than the room of the terminal, so large frames do not cross the terminal. If the terminal size changes and then stays the same for a quarter second, the view asks again with the new size. The last row is the status line. It shows the target, the frame size, the protocol and the menu key.

### Keys and the mouse

Text and keys go to the target app. Ctrl, Alt and Shift become Control, Option and Shift in the guest. A terminal with the kitty keyboard protocol also sends Super as Cmd. A paste goes to the guest as typed text.

A click in the picture is a click at that point of the guest. Two quick clicks are a double click. A press, a move and a release are a drag. The wheel scrolls, and Shift with the wheel scrolls sideways. If the terminal reports the mouse in pixels, the view uses the pixels. Otherwise it uses the center of the cell.

### The menu key: Ctrl-]

A terminal keeps most Cmd combinations for itself. Press Ctrl-] to open the menu. The next key then goes to the Mac with Cmd:

```text
q  Cmd-Q     w  Cmd-W     c  Cmd-C     v  Cmd-V     Z  Cmd-Shift-Z
Tab  Cmd-Tab      Space  Cmd-Space      Alt-h  Cmd-Option-H
Ctrl-E  Cmd-Option-Esc (Force Quit Applications)
Ctrl-]  send Ctrl-] to the Mac
Ctrl-L  draw the picture again
Ctrl-C  leave the view
Ctrl-D  leave the view
Esc     close the menu
```

Outside the menu, Ctrl-C goes to the guest as Control-C.

### Leave the view

Press Ctrl-] and then Ctrl-C or Ctrl-D. The view stops the frame stream and gives the terminal back as it was. The session and its app keep running. To see it again, run the same `paddock mac view` command. To end the session, run `paddock app stop NAME`. A SIGTERM or SIGHUP also ends the view and restores the terminal.

### tmux and GNU screen

tmux stops image escapes unless `allow-passthrough` is on. If it is off, the view says so and does not draw kitty or iTerm2 images. To turn it on, run this in tmux 3.3 or later:

```sh
tmux set -g allow-passthrough on
```

The view then wraps the kitty and iTerm2 escapes for tmux. In tmux the view waits up to one second for the answer of the outer terminal to the kitty query, because tmux answers the other queries itself. tmux does not know about these images. If you switch windows or panes, press Ctrl-] and then Ctrl-L to draw the picture again. tmux 3.4 built with sixel draws sixel images itself and needs no passthrough. GNU screen passes no image escapes, so run the view outside screen.

### Limits

- The view does not read the frame stream of the guest agent (`frames.subscribe`) yet, so on a Mac it stops at the start and says so. Until then, `paddock mac screenshot` shows the guest (`paddock help computer-use`). `--fake-source` tries the terminal side.
- Kitty images in tmux: tmux moves its own cursor, and the image goes past tmux to the outer terminal. The view does not follow pane moves, and the outer cursor can be out of step with tmux. Press Ctrl-] and then Ctrl-L to draw the picture again.
- Terminal replies that come late (for example a slow kitty answer in tmux) are dropped, never sent to the guest as keys. If you type Alt-] or Alt-P without the kitty keyboard protocol, the key reaches the guest after a short wait of about 40 ms.
- Sixel uses a fixed palette of 216 colors, so gradients show bands. Sixel also needs more bytes and time than the other two protocols.
- `--fake-source` shows a moving test picture instead of a guest. It works on every system and tests the terminal side only.
- paddock has not run the view against a guest on a Mac yet.
