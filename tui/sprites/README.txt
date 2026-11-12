paddock toon sprites (direction C "Rooms")

Made with the same three.js toon materials, outlines and lights as the
launch video and style-frames/hybrid-16x9.png. All PNG files have a
transparent background. Animation rate for all loops: 8 fps.

Characters
  character-COLOR-STATE-N.png    512 x 512 px (256 x 256 at 2x), N = frame
  COLOR   harness
  orange  claude
  blue    codex
  purple  gemini
  green   copilot
  pink    grok
  teal    opencode
  gray    a desktop app, or an unknown harness
  STATE    frames  what it shows
  idle     8       gentle bob, blink on frame 6
  work     8       typing at a laptop, turned towards it
  wait     8       looks at the viewer, amber "!" bubble (an approval waits)
  sleep    4       eyes closed, "z" letters rise
  blocked  4       red stop badge
  7 colors x 32 frames = 224 files.

Rooms (no character)
  room-cream.png, room-blue.png, room-lilac.png, room-mint.png   512 x 512 px
  room-cream-screen.png   512 x 512 px, the laptop faces the viewer with an
                          empty light screen for the real app icon
  room-front.png, room-front-screen.png   512 x 512 px, only the desk and
                          laptop. Draw order: room, character, front layer.
                          Inside a room, use the idle, wait, sleep or blocked
                          states; the work state brings its own laptop.
  rooms.json   per room:
    character.feet        pixel where a character's feet go
    character.spriteScale scale a character PNG by this to fit the room
                          (the character renders use the same camera angle)
    character.heightPx    character height in the room, in pixels
    screen.corners        (room-cream-screen.png only) the screen quad,
                          top-left, top-right, bottom-right, bottom-left, in px

Terminal pixel art
  term/character-COLOR-STATE.json   24 x 24 px per frame
  {"w":24,"h":24,"fps":8,"harness":"...","frames":[ rows of [r,g,b] or null ]}
  frames[f][y][x] is one pixel, null is transparent. Draw two pixels per cell
  with the upper half block, so one frame is 24 columns x 12 rows. Each frame
  has a clean 1 px outline in ink (43,33,64). The "!" bubble of the wait state
  is drawn as pixel art in the same amber, because it is too small to survive
  the downscale.

Source: src/sprites.js and export-sprites.mjs (node export-sprites.mjs).
