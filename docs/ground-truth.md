# Ground truth from IRIX

The port is checked against the real `ep` running on IRIX 5.3 in the iris
emulator. Every IrisGL call ep makes is recorded and compared with the port's
own draw stream. The SGI binary, the traces and the decompiled code live in
the git-ignored `re/` directory; this page describes how to produce them.

## Call tracer

`tools/irix/gen_tracer.py` generates `eptrace.c` from the IrisGL prototypes and
the list of functions ep imports. Each generated wrapper:

- logs the call as a binary record: u16 opcode, u16 payload length, then the
  arguments in MIPS (big-endian) order;
- logs pointer arguments by value when their size is known: 2, 3 or 4 floats
  for `v2f`/`v3f`/`n3f`/`c4f`, 16 for matrices, and `np` floats for `lmdef`;
- forwards the call to the real function, looked up with `dlopen`/`dlsym`.

Calls libgl makes to itself are not logged. Records are grouped into frames,
with `swapbuffers` marking the end of each frame. The wrapper for `time()`
returns a fixed value, so the `srand48` seed (and with it the whole show) is
reproducible. Before `winopen` the tracer calls `foreground()`, so ep doesn't
fork into the background and the caller can wait for it to finish.

Build and run on IRIX (IDO cc):

    cc -O -c eptrace.c && ld -shared -o libeptrace.so eptrace.o -lc
    DISPLAY=:0 EPTRACE_OUT=/usr/tmp/ep.trace EPTRACE_FRAMES=300 EPTRACE_TIME=12345 \
      _RLD_LIST=$PWD/libeptrace.so:DEFAULT /usr/demos/bin/ep [-q|-S|-a < script]

Decode on the Mac with `tools/irix/eptrace_dump.py OPS_JSON TRACE`. Add
`--summary` for per-call counts, or `--frames A:B` to print a range of frames.

Two runs with the same seed produce byte-identical traces, mouse position
included as long as the pointer doesn't move.

## Random numbers

IRIX `drand48` is the standard 48-bit LCG (a = 0x5DEECE66D, c = 0xB; `srand48`
loads `seed << 16 | 0x330E`). `tools/irix/drand48_fixture.c` prints IRIX output
for a seed. A Python clone matches all 5,000 values from seeds 0, 1, 12345,
1730000000 and -1 bit for bit.

ep calls `srand48(0)`, then `srand48(time(NULL))`.

## Built-in script

`ep -o` echoes the internal script as it parses it: each directive line shows
the parsed values followed by the original text. Removing the parsed prefix
gives the original "mello" script (by Dave Tristram). Feeding that text back
with `ep -a` reproduces the built-in show's trace byte for byte.

## Which drawing path the original uses

ep checks the hardware once at startup. The low-end fallbacks apply only when
`getgdesc(GD_BITS_NORM_DBL_RED)` is 0 (colour-index only) or below 4 (Indigo
entry graphics). They turn off the z-buffer and blending. The emulated Indy
(XL24) reports 4, so it gets the full path, the same as Indigo2, O2 or
Octane-class machines.

After that, the drawing style comes from panel toggles (actuators), not from
the hardware:

| Toggle        | Key | Actuator id | Mello default |
|---------------|-----|-------------|---------------|
| outline       |     | 18          | on            |
| ribbons       | R   | 21          | off           |
| z-buffer      | Z   | 26          | off           |
| light         | L   | 27          | off           |
| full (screen) |     | 29          | on            |

With the defaults, the show is flat squares: each history slot is a
0.2 × 0.2 polygon in the slot's colour, outlined in the complementary colour
(1 − r, 1 − g, 1 − b) at line width 1. No lighting, tmesh or z-buffer calls
appear in 3,600 traced frames of the default, quad (`-q`) or screensaver (`-S`)
modes.

Turning ribbons, z-buffer and light on (script `mello-rzl`) switches to a lit
triangle-mesh ribbon:

- `zclear` each frame;
- one `bgntmesh` per strand;
- a per-slot `lmdef` material followed by `lmbind`;
- two vertices per slot, each transformed by that slot's matrix.

## What the emulator does not show faithfully

IrisGL computes geometry, lighting and colour on the CPU for the Indy, so the
draw stream is the same as on any full-path machine. Only the final pixels
differ. In double-buffered RGB mode the XL24 has 12 bit-planes (4 bits per
channel, dithered), whereas O2- and Octane-class machines show 8 bits per
channel. The port renders at full colour precision. Emulator screenshots
therefore serve as a reference for geometry and motion only; colours are
checked numerically against the trace.

The IRIX 6.5 build (1998) has the same strings, script and code paths as the
5.3 build (1994); the differences are toolchain sections only.

## Checking the Rust port

`crates/ep-core` replays the traces directly: `electropaint_core::trace` decodes EPT1
with `eptrace_ops.json`, runs the simulation with the same flags, script and
seed, and compares the two call streams frame by frame. The integration
tests (`cargo test -p electropaint-core --test traces`) do this for every trace and
skip any that are missing (`EP_REQUIRE_TRACES=1` makes that a failure). For
one trace at a time, with the first divergence shown in context:

    cargo run -p electropaint-core --example trace_diff -- re/tracer/ep_v_zl.trace \
        --script re/scripts/mello-zl.ep [--quad] [--saver] [--frames N]

What is compared: the drawing calls (matrix, colour, vertex, primitive,
lighting, buffer, pattern and projection calls, `swapbuffers`), the five
channel swatches `mapcolor(0x300..0x304)`, and `srand48`/`drand48` with
the drawn values. Window management, event, query and panel calls are
skipped, as are the panel library's own `defpattern(1, …)` and its colour
map. Integers (all `rotate` angles included) and floats must match exactly:
the comparison lets floats within 1e-5 relative through but counts those
whose bits differ, and the tests require that count to be zero (`trace_diff`
prints it, with the largest relative error). Where the
original passes uninitialised stack memory (the ribbon material's ambient
factor), the simulation logs NaN and the comparison accepts any value.

Inputs are replayed, not simulated: a window-system REDRAW happens on tick 1
in some captures and tick 2 in others (rendering.md §2), so the comparison
delivers it where the trace's main loop reads one. Comparison stops before
the first frame with a mouse event.

Current result: every trace matches over its full length with all floats
bit-identical.

| trace | flags / script | frames |
|---|---|---|
| `ep5`, `ep200`, `ep200b` | default | 5, 200, 200 |
| `ep_long` | default | 3600 |
| `ep_a` | `-a < mello.ep` | 200 |
| `ep_q` | `-q` | 300 |
| `ep_S` | `-S` | 300 |
| `ep_rzl` | `mello-rzl` | 300 |
| `ep_v_light`, `_zl`, `_smear`, `_smoothsmear`, `_smearfade`, `_back`, `_tri`, `_fat` | `mello-<name>` | 300 each |
| `ep_v_rzltri` | `mello-rzltri` | 261 (a mouse click left full screen at frame 261) |
| `ep_v_rzl-q` | `-q`, `mello-rzl` | 189 (libgl crashed in frame 188; its recorded prefix matches) |

## Screen grabs

Setting `EPTRACE_PAUSE_AT=N` makes the tracer hold the picture for
`EPTRACE_PAUSE_SECS` seconds right after swap N. While it holds,
`/usr/tmp/eptrace.paused` exists, and IRIX's `scrsave` can grab the screen
(`re/scripts/grab.sh FRAME OUT.rgb [ep args]`; set `EPSCRIPT=file` to feed a
script with `-a`). `electropaint --snapshot` renders the same frame offscreen
at 1280x1024 for comparison.

The emulated XL24 shows 4 bits per channel (16 levels), and the port 8, so
colours are compared after reducing the port's image to 4 bits. Shape
interiors are compared separately from edges, because the port antialiases
its edges.

| frame 300 unless noted | shapes | interior colour within one 4-bit step |
|---|---|---|
| default show, frame 600 | all differing pixels on edges except 2 | 99.5% |
| light (27) | 1,017 differing pixels off the edges (dark shading) | 99.6% |
| smear + fade (23, 24) | whole screen | 97.7% |

The remaining colour differences fit the XL24's dithering between
neighbouring levels. The smear + fade comparison settled how `color()`
behaves in RGB mode (rendering.md §8.1).

During one of these grabs (plain smear) the emulator froze and had to be
restarted; the cause is unknown, and the other grabs worked.

