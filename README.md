# electropaint

A port of SGI's electropaint (`/usr/demos/bin/ep`, IRIX 5.3) to macOS, written
in Rust with Metal. It runs the original's built-in "mello" show, along with
every drawing mode and control the original has.

The simulation is a re-implementation from the reverse-engineered binary.
Its IrisGL call stream matches recordings of the real program running on
IRIX bit for bit: 18 traces, 3,600 frames in the longest. In screen grabs
from IRIX the shapes land on the same pixels as the port's, and the colours
agree to within the emulated Indy's 4-bit colour depth.
`docs/ground-truth.md` describes how the recordings were made and checked.

Apple Silicon and macOS 14 or later.

## Running

    cargo run --release -p electropaint

This fills the main display; any key or click quits. Other options:

| option | |
|---|---|
| `--all-displays` | one independent show per display |
| `--windowed` | a normal window; input goes to electropaint (below) |
| `--controls`, `-c` | the control panel, kept open |
| `--quad` | four mirrored strands (ep's `-q`) |
| `--script FILE` | run an ep script instead of the built-in show (ep's `-a`) |
| `--seed N` | fixed random seed (the original seeds from the clock) |
| `--snapshot FILE.png --frames N --size WxH` | render offscreen and save a picture |

With `--windowed` or `--controls` the show takes input as ep did: keys are
ep's shortcuts (listed under [Controls](#controls)) and Esc quits. A click on
the show toggles ep's full-screen state, and leaving full screen brings up the
control panel.

### Timing

The original advanced one step per 60 Hz retrace, and everything (script
timings, the random drift of every control, strand growth) counts steps. The
port keeps 60 steps a second on any display. On a faster display, the
refreshes between steps show pictures that ep's own drawing code produces
partway through the step. In smear and fade modes, which build trails on
screen, those refreshes repeat the last step.

## Controls

The control panel has the same controls, ranges and behaviour as ep's own
panels. The built-in show moves many of them by itself: its script lets
twist, wheel, spin, flip, arm, wrist, outline and the first colour channel's
hue and lightness drift at random between limits.

ep draws a *strand*: a chain of up to 127 tiles, one per *slot*. Each step
the strand grows a new slot at its head, which records the current control
settings, and the oldest slot drops off. A setting therefore travels down
the strand as the head moves on, which is what makes the trails curl.

### Toggles and buttons

| control | key | what it does |
|---|---|---|
| outline | O | Draws each tile's edge in the complementary colour. Recorded per slot. |
| fat lines | I | Outlines 3 pixels wide instead of 1. Recorded per slot. |
| fill | U | Fills the tiles. With fill off and outline on, only the edges show. Recorded per slot. |
| triangles | T | Triangles instead of squares. |
| ribbons | R | Replaces the tiles with one continuous ribbon through the strand, each band in a slot's colour. With light also on, the ribbon takes the background colour and disappears (a quirk of the original). |
| smear | Q | Stops clearing the screen, so every tile drawn stays and the strand paints trails. |
| smooth | M | With smear on, draws each step twice so that both of the screen's buffers hold the same trails. The animation runs at half speed. No effect without smear. |
| fade | | With smear on, each step repaints one pixel in every 4×4 block in a fixed dither order, so trails dissolve over 16 steps. On the Indy the dissolve goes to deep blues. ep leaves this toggle off its RGB panel and gives it no key; on IRIX only a script could turn it on. |
| back | E | Cycles the background through the hues at full saturation, at the rate set by outr. Off, the background is black. |
| z-buf | Z | Depth testing: nearer tiles hide farther ones. Without it, tiles are drawn newest first, so older tiles paint over newer ones wherever they overlap. |
| light | L | Lights the tiles with two distant lights from opposite directions, so each tile brightens or darkens with the way it faces. amb and shn only matter while this is on. |
| stop | S | Freezes the show. |
| full screen | F | ep's full-screen state. On IRIX it switched between full screen and a window with the panels showing. Here the window stays as it is; leaving full screen brings up the panel in `--windowed`. |
| auto | | Switching it off stops all random drift. In the built-in show only spin and flip start again, because only their script sequences loop. |
| reset | | Puts the controls back to their starting values, zeroes the accumulated wheel, spin and flip angles, and recentres the position. Leaves stop, full screen, auto and the channel buttons alone. |
| exit | Esc | Quits. |

### Sliders

Each tile is placed by these moves, in order: by the position, turned by
spin, out along the arm, tilted by flip, along the wrist, then scaled by
size. Only twist and zoom carry on from one slot to the next: each slot's
starting frame is the previous one's turned by twist and moved by zoom.

| slider | range | what it does |
|---|---|---|
| zoom | −0.5 to 0.5 | Distance between successive slots along the strand's axis. |
| twst | −200 to 200 | How far each slot is turned from the one before it, in degrees. |
| spd | 0 to 1 | Time scale. At 1 the strand grows a slot every step; lower values blend positions in between. It also scales wheel, the sliders' own oscillation, the background hue and the colour channel clock. |
| n | 1 to 127 | How many slots are drawn. |
| amb | 0 to 1 | With light on, how much of a tile's colour shows whichever way it faces. At 0, tiles facing away from both lights go nearly black. |
| shn | 0 to 1 | With light on, how sharp the white highlights are (0 turns them off). |
| whl | −60 to 60 | Rolls the whole strand about the horizontal axis, by this many degrees per step. |
| spn | −20 to 20 | Added to the spin angle each time a slot is added: swings the arm round the strand's axis. |
| flp | −20 to 20 | Added to the flip angle each time a slot is added: tilts the tiles. |
| arm | −3 to 3 | Distance from the strand's axis to the tile, before the flip. |
| wrst | −3 to 3 | A second offset, after the flip. |
| sz | 0 to 10 | Tile size. |
| position | | Moves the head of the strand; older slots keep where they were, so the strand trails after it. |

Each slider has the same five parts:

- **value**: the setting itself.
- **low** and **high**: the window the value may move in, both for the
  script's random drift and for the slider's own oscillation. Dragging low
  moves the whole window and keeps its width; dragging high changes the
  width.
- **rate**: makes the value sweep through its window by itself, faster for
  higher rates.
- **bounce**: when the sweep reaches an end of the window it turns back.
  Unchecked, it wraps round to the other end.
- **R**: the first press restores the window, rate and value. A second
  press, when the value is already back at its start, also zeroes the
  accumulated angle of whl, spn and flp.

### Colours

The tile colour comes from one of five colour channels at a time. Each
channel has a hue and a lightness; the outline uses the opposite hue and
lightness.

| control | what it does |
|---|---|
| outa | Outline opacity. |
| baka | Background opacity. It goes into the clear colour, but ep's clear doesn't blend, so it has no visible effect. |
| outr | How fast the background hue cycles when back is on (−20 to 20). |
| hue / lightness | Which of each channel's sliders the panel shows, as ep's panel did. Both are ep sliders with the five parts above. |
| active | Lights the channel in use. In ep, holding one down made that channel the one in use; the port's panel button clicks, so it selects the channel only for a moment. |
| colour | The channel's current colour. |
| alpha | Opacity of the tiles while this channel is in use. |
| dwell | The share of time each channel is used. Moving one adjusts the others so they keep summing to 1. |
| channel rate | How fast the channel clock runs. At 0 (the default) one channel stays in use. |

## Screen saver

    ./scripts/make-saver.sh

builds `target/saver/electropaint.saver`. Double-click it to install, or copy
it into `~/Library/Screen Savers`.

## Layout

| path | |
|---|---|
| `crates/ep-core` | the simulation: script, sequencer, actuators, strand drawing, an IrisGL emulation, trace comparison |
| `crates/ep-metal` | the Metal renderer and the show driver (pacing, in-between pictures) |
| `crates/electropaint` | the app and its control panel |
| `crates/ep-saver` | the screen saver |
| `docs/spec` | the reverse-engineered behaviour, in our own words |
| `tools/irix` | the IRIX-side call tracer generator and helpers |

SGI's binaries, decompiled code, the original script text and the IRIX
recordings stay in the git-ignored `re/` directory. Tests that compare against
recordings skip themselves when `re/` is absent.

## Tests

    cargo test --release --workspace

## Thanks

Thanks to David Tristram, whose name is on the original electropaint's
control panel and who wrote the "mello" show that this port runs.
