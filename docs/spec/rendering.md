# electropaint (IRIX 5.3 `ep`): rendering specification

Source binary: `/usr/demos/bin/ep` (IRIX 5.3, MIPS-I o32, big-endian, IrisGL).
Evidence: Ghidra export `re/ghidra/ep-irix53/` plus a direct disassembly of the
same binary (`re/tools/disasm.py`, capstone). Data dumps come from
`re/tools/dumpwords.py` and `re/tools/globals.py`. Addresses are IRIX 5.3
virtual addresses. Where the decompiler and the machine code disagree, this
document follows the machine code; the cases are noted.

Notation:

* `f32(x)` means rounding to IEEE single, `f64` to double. The original is K&R C
  compiled for o32. Float arguments are promoted to double, and many
  expressions mix single and double arithmetic. The exact mix is given where it
  could change a result bit.
* `trunc(x)` is C float-to-int conversion. **Correction to the brief:** every
  `ROUND`/`FLOOR` pair in the Ghidra output is the compiler's inline
  `(int)` cast. The code saves FCSR, sets `RM = (fcsr | 3) ^ 2`, which is RM=01
  (round toward zero), runs `cvt.w.{s,d}`, and restores FCSR (e.g. drawit
  `0x4117f0..0x411810`). So every float→int conversion in ep **truncates toward
  zero**. It never rounds to nearest.
* `on(b)` is the panel toggle test used everywhere:
  `0.5*f64(b.min) + 0.5*f64(b.max) < f64(b.val)`, with `min` at `+0x50`, `max`
  at `+0x54` and `val` at `+0x44` of the actuator. Toggle buttons have min 0 and
  max 1, so `on(b)` is `val > 0.5`.
* `S(name)` is the effective value of an "ep slider" built by `mkepslider`
  (0x414488): `*(float*)(*(*(slider+0xa0)+0x14)+0x48)`, i.e. the multislider's
  current bar. Plain pnl dials/sliders use `+0x44` (written `D(name)`).
* "slot" means a history index 0..127. `slot(k) = ((k % 128) + 128) % 128`
  (C remainder, then made positive). This matches the `andi 0x7f` /
  `-0x80` / `+0x80` sequences.

---

## 0. Validation against IrisGL traces (`re/tracer/*.trace`)

I checked the spec against traces of the real binary
(`python tools/irix/eptrace_dump.py re/tracer/eptrace_ops.json re/tracer/<t>.trace --frames a:b`;
the decoder starts a new frame at each `swapbuffers`). Confirmed:

* Frame 0 setup matches §2 call for call: `deflight` tables (identical floats),
  `RGBmode`, `blendfunction(4,5)`, `subpixel(1)`, `gconfig`, `shademodel(0)`,
  `concave(0)`, `glcompat(0,0)`, `perspective(300, 1, 0.01, 1e6)`,
  `loadmatrix(I)`, `polarview(10,0,0,0)`, `lsetdepth(0, 8388607)`,
  `c4f(0,0,0,1)`, `clear`, `swapbuffers`, `clear`.
* On the emulated XL24, `getgdesc(9)` returns 4 (so `is_indigo = 0` and
  `has_zbuffer = 1`), and `getplanes()` in RGB mode returns **12**, which gives
  `ncolors = 128` and `basecolor = 640`. The initial event is INPUTCHANGE, not
  REDRAW. The script's `full = 1` then runs `fullscreen()`, which issues
  `perspective(300, 1.25, 0.01, 10000)` for 1280×1024.
* The per-slot drawit sequence matches §5. One frame looks like this:
  * `c4f(bkg)`, `clear`, `pushmatrix`, `rotate(wheel·10, 'x')`
  * nlimit × `translate(0,0,−Z/2)` (Z = 0.06 gives −0.03; nlimit = 40)
  * then for each slot:
    * `c4f(col)`, `linewidth(1)`, `pushmatrix`, `translate(X,Y,0)`
    * `rotate(spin,'z')`, `translate(0,ARM,0)`, `rotate(flip,'y')`
    * `translate(WR,0,0)`, `scale(1.2,1.2,1)`
    * the square as `bgnpolygon`/4×`v2f`
    * `c4f(outline)` and the square as `bgnclosedline`
    * `c4f(col)`, `popmatrix`, `rotate(twist,'z')`, `translate(0,0,Z)`
  * a final `popmatrix` and `swapbuffers`.
* `ep_q.trace` confirms the quad-copy order: `scale(1,−1,1)`, then
  `rotate(1800,'z')`, then `scale(1,−1,1)`, with each copy re-issuing the full
  local chain.
* The outline colour in the trace is exactly the per-channel complement of the
  fill (e.g. fill (0.150421143, 0.626716256, 1) gives outline (0.849578857,
  0.373283863, 0)). This agrees with §5: for s = 1,
  `HLS(h+½, 1−l, 1) ≡ 1 − HLS(h, l, 1)`. The HLS trapezoid satisfies
  `f(h+180°) = 1 − f(h)`, and the m1/m2 values for `1−l` are the complements of
  those for `l`. **Implement it literally with the second `hls_to_rgb` call,
  not as `1−rgb`**, so the float rounding matches.
* Across all captured runs (default mello script, `-q`, `-S`, `-a`; 3600
  frames) **no** `lmbind`, `n3f`, `bgntmesh`, `zbuffer`/`zclear`,
  `frontbuffer` or `setpattern` call appears. Lighting, z-buffer, ribbons,
  smear and fade are reached only through the panel toggles (light "L",
  z-buf "Z", ribbons "R", smear "Q"; the RGB build never adds the fade button,
  so fade has no key and is reached by a script; actuator ids per the panel spec,
  e.g. outline = 18, full = 29). The variant traces in §0.1 cover them.

### 0.1 Variant traces: lit, z-buffer, ribbon, smear, fade, background, triangle and fat-line paths

These are the mello script plus `actset: <id>, 1.0` lines
(`re/scripts/mello-<name>.ep` → `re/tracer/ep_v_<name>.trace`, `time() = 12345`,
300 frames; `ep_rzl.trace` is ribbons + z-buf + light). `actset` goes through
`setact` @0x40d824, which writes `val` and **does call the actuator's
callback** (`+0x90`, then `+0x94`, `+0x98`). Script toggles therefore trigger
`setsmear`, `setsmooth` and `setzbuf` exactly as key presses do. `actset` also
reaches actuators that are not visible in the panel (fade, id 24), because
`acttable` holds every actuator `ep_mkact` created.

| variant (ids) | observed | vs spec |
|---|---|---|
| light (27) | Per drawit, after `c4f(bkg)`/`clear`: `lmbind(1105,5)`, `lmbind(1000,1)`, `lmbind(1106,6)`, `lmbind(1200,1)`, then `pushmatrix`, `rotate(wheel)`. Per slot: `lmdef(0,1,17,[2 r·a g·a b·a 3 r g b 4 1 1 1 5 S 7 α 0])`, `lmbind(1000,1)`, `linewidth`, the transform chain, **`n3f([0 0 1])` after `scale`**, polygon; then `lmdef` with the outline colour, `lmbind`, closed line; then `lmdef` with the fill colour again. With amb = 0 and shiny = 1: AMBIENT = 0 and SHININESS = 128. No `c4f` inside the slot loop. | ✓ §5.2–5.3 |
| zl (26, 27) | `zbuffer(1)` + `zclear()` once (from the `setzbuf` callback), then `zclear()` right after the frame's `clear()` and before `uselight`, every frame | ✓ §5, §8 |
| smear (23) | `setsmear` callback: `c4f(bkg)`, `clear`, a drawit with zero slots (`nlimit` was not yet computed), `frontbuffer(1)`, `backbuffer(1)`. After that, no per-frame `clear`; one `drawit` + `swapbuffers` per step. | ✓ §8 |
| smoothsmear (22, 23) | Smooth is set first (smear off, so no-op), then smear with smooth on: **no** `frontbuffer`/`backbuffer` calls. Each step: drawit, swap, (panel `winset`), drawit, swap, with the same wheel/slot state in both halves. | ✓ §8 |
| smearfade (23, 24) | Each step: `defpattern(2,16,&tex)`, **`color(0)`**, `setpattern(2)`, `clear()`, `setpattern(0)`, then drawit, swap. Pattern words are not captured (pointer only). | call sequence ✓; `color()` **is** issued in RGB mode |
| back (25) | `c4f(bkgRGBA)` before each clear cycles hue. **All 297 background colours match the §3 bhue recurrence plus §7.1 `hls_to_rgb` bit-exactly** (checked with a single/double-precision model of both). | ✓, and confirms the §7.1 precision model |
| tri (317) | Fill and outline vertices are `(0,0)`, `(0.2,0)`, `(0.1,0.173205078)` | ✓ §1.2 |
| fat (19) | `linewidth(3)` once per slot (40 per frame) | ✓ |
| rzltri (21, 26, 27, 317), rzl (21, 26, 27) | `zclear`, `pushmatrix`, `rotate(wheel)`, `pushmatrix`, `bgntmesh`, then per slot `lmdef(material)`, `lmbind(1000,1)`, `linewidth`, the chain, `v2f([0 0])`, `v3f([0.2 0 0.1])`, `popmatrix`, `rotate(twist)`, `translate(0,0,Z)`. **No `uselight`, no LMODEL/LIGHT binds, no `n3f`, no `c4f` in the loop.** | ✓ §6 |
| rzl-q (21, 26, 27, -q) | Per frame: 4×`bgntmesh` and 1×`endtmesh` (at the very end). Between copies: `rotate(1800,'z')`, `scale(1,−1,1)`, `rotate(1800,'z')`. Copies 2 and 3 use `rotate(−twist)`. The run segfaulted at frame 189 inside Indy libgl's software tmesh rasteriser (`_tmesh_doit0`); frames 0–188 are usable. | ✓ §6 |

Resolved by these traces:

* **Ribbon "garbage" material.** The uninitialised ambient multiplier is
  ≈2.525e−29 in 92–99 % of slots. In whole frames at a regular interval
  (rzl: frames 34, 116, 198) it is ≈3.675e+19, and a few slots get
  denormals. SHININESS is always 0. **More important:** in a ribbon-only
  session `uselight` is never called, so LMODEL stays bound to 0 and **IrisGL
  lighting is disabled** (`lmbind` man page). The `lmdef`/`lmbind(MATERIAL)`
  calls do nothing, and the ribbon vertices take the **current colour**, which
  is the `c4f(bkgRGBA)` main issues before each clear. With smear off, lit
  ribbons are therefore drawn in the background colour and are invisible. See
  §6 for the port rule.
* **`fadebackground` and `color()` in RGB mode.** Confirmed: `color(bkg)` is
  issued (here `color(0)`). An Indy screen grab settled what the hardware does
  with it (§8.1).
* Quad ribbons do issue 4 `bgntmesh` per 1 `endtmesh`. The rendering result is
  still open (§11.3).

Capture artefact: in `ep_v_rzltri.trace` a mouse event (device 103) around
call 178107 made ep leave fullscreen (`regularscreen`: `winposition(8,991,…)`,
`perspective(300,1,…)`, `curson`, panel redraw). Ignore it when comparing.

**Target fidelity (user decision):** emulate the highest-fidelity hardware
class (Indigo2 Extreme / O2 / Octane): 8 bits per channel double-buffered RGB,
no 4-bit dither, z-buffer available, full lighting/ribbon/z-buffer paths
supported. The XL24 emulator's 4/4/4 dither is **not** the target.

---

## 1. Global configuration and constants

### 1.1 Feature flags (`.data`, initial values)

| global | addr | init | meaning |
|---|---|---|---|
| `rflag` | 0x10000000 | 0 | `-r` read panel script |
| `aflag` | 0x10000004 | 1 | run internal/stdin ep script (auto animation) |
| `bflag` | 0x10000008 | 1 | internal script (`defaultScript` ptr @0x10000410 → text @0x100010d0) |
| `sflag` | 0x10000014 | **1** | single strand (default). `-q` sets `sflag=0`, which gives four mirrored strands |
| `qflag` | 0x10000018 | 0 | |
| `tflag` | 0x1000001c | 0 | `-t` load `./*.rgb` textures |
| `is_saver` | 0x10000020 | 0 | `-S` |
| `is_indigo` | 0x10000024 | 0 | set if `getgdesc(GD_BITS_NORM_DBL_RED) < 4` |
| `is_ci_only` | 0x10000028 | 0 | set if `getgdesc(GD_BITS_NORM_DBL_RED) == 0` |
| `rgb_mode` | 0x1000002c | 1 | |
| `has_lighting` | 0x10000030 | 1 | |
| `has_tmesh` | 0x10000034 | 1 | ribbons are possible |
| `has_textures` | 0x10000038 | 1 | texture panel exists (textures load only with `-t`) |
| `has_zbuffer` | 0x1000003c | 1 | forced to 0 if `is_indigo` or `is_ci_only` |

### 1.2 Geometry tables (`.data`)

| symbol | addr | float values | hex |
|---|---|---|---|
| `square[0]` | 0x10000058 | (-0.1, -0.1) | bdcccccd bdcccccd |
| `square[1]` | 0x10000060 | ( 0.1, -0.1) | 3dcccccd bdcccccd |
| `square[2]` | 0x10000068 | ( 0.1,  0.1) | 3dcccccd 3dcccccd |
| `square[3]` | 0x10000070 | (-0.1,  0.1) | bdcccccd 3dcccccd |
| `ep_trinormal` | 0x10000078 | (0, 0, 1) | 00000000 00000000 3f800000 |
| `triangle[0]` | 0x10000084 | (0, 0) | 00000000 00000000 |
| `triangle[1]` | 0x1000008c | (0.2, 0) | 3e4ccccd 00000000 |
| `triangle[2]` | 0x10000094 | (0.1, 0.173205078) | 3dcccccd 3e315cac |

The 4x4 identity at 0x1000009c (used by `skew`, not on the render path) and the
4x4 identity at 0x100002bc (loaded by `loadident` @0x40eab4 via `loadmatrix`)
are plain identity matrices.

### 1.3 Initial colour globals

| symbol | addr | init |
|---|---|---|
| `bkg` (int; low short passed to `color`) | 0x10000040 | 0 |
| `bkgRGBA` | 0x10000044..0x50 | (0, 0, 0, 1) |
| `outlinecolRGBA` | 0x100002fc..0x308 | (0, 0, 1, 1) |
| `colRGBA` | 0x1000030c..0x318 | (1, 0, 0, 1) |
| texcoord scratch | 0x1000031c..0x338 | (0,0),(0,1),(1,1),(1,0) |

### 1.4 Slider ranges and initial values

These come from `mkepslider` calls in `defpanel` (`init` is stored at `+0x44`
and in the bar's `+0x48`). The table gives the stored `(+0x54, +0x50)` pair as
`(lo, hi)`. Only the ranges matter for rendering; animation and the UI are
covered elsewhere.

| slider | label | lo | hi | init |
|---|---|---|---|---|
| `zoomslider` | zoom | -0.5 | 0.5 | 0 |
| `twistslider` | twst | -200 | 200 | 0 |
| `speedslider` | spd | 0 | 1 | 1.0 |
| `nslider` | n | 1 | 127 | 32 |
| `ambientslider` | amb | 0 | 1 | 0 | (only when `has_lighting`) |
| `shinyslider` | shn | 0 | 1 | 1.0 |
| `wheelslider` | whl | -60 | 60 | 0 |
| `spinslider` | spn | -20 | 20 | 0 |
| `flipslider` | flp | -20 | 20 | 0 |
| `armslider` | arm | -3 | 3 | 0 |
| `wristslider` | wrst | -3 | 3 | 0 |
| `sizeslider` | sz | 0 | 10 | 1.0 |
| `texture{t,s}slider` | t, s | -0.01 | 0.01 | 0 |
| `texture{s,t}maxslider` | S, T | 0 | 10 | 1.0 |
| `maprateslider`/`maprangeslider` | rat/rng | CI only | | |

`speed ≤ 1` and `n ≤ 127` (unless the script drives them past these ranges), so
`n` advances by at most 1 per frame, and the strand never reuses a history slot
within one frame.

Toggle defaults set by `defpanel` (val at `+0x44`; everything else is 0):
`fillbutton = 1`, `trianglebutton = 0` (the **square** is the default shape),
"no texture" radio = 1, alpha dials `adial[i] = 1`, `alphaoutdial = 1`,
`alphabackdial = 1`, `huebackratedial = 0.2` (range ±20). The internal default
script ("mello script", Dave Tristram) sets fullscreen on, outline on,
zoom 0.06, twist 60, size 1.2 and n 40, then animates wrist, hue/lightness of
channel 0, wheel, spin, flip, arm, twist and outline. It **does not** turn on
light, z-buf, smear, fade, ribbons, background or triangles. The out-of-the-box
look is therefore unlit, alpha-blended squares (fill + outline) with no
z-buffer, cleared every frame.

---

## 2. Window, projection and global GL state (`main` @0x40eaec)

Initialisation order, as executed:

1. `getgdesc(GD_YPMAX)` → `regw = regh = YPMAX − 29`, `regx = regy = 5`.
2. `srand48(0); srand48(time(0))`. drand48 is only used by the animation code
   (`exprand`, `setacttargets`). **No randomness is used in rendering.**
3. Allocate 25 history arrays of 128 × 4 bytes with `pnl_alloc` (calloc), so all
   start at **0**: `x y dzoom arm wrist dtwist flip spin outline fatline fill size
   maprange hue alpha light alphaout shiny ambient textureindex texturet textures
   dtexturet dtextures texturetmax texturesmax`. `wheel = 0`.
4. Hardware probe: `getgdesc(9 = GD_BITS_NORM_DBL_RED)`. 0 → `is_ci_only`;
   `< 4` → `is_indigo`; either one → `has_zbuffer = 0`.
5. Window. Non-saver: `prefposition(5, 5+regw−1, 5, 5+regh−1)`, `winopen("ep")`,
   `keepaspect(1,1)`, `winconstraints`, `winattach`. Saver (`-S`):
   `foreground(); noborder(); winopen("ep")`.
6. `doublebuffer()`.
7. If `has_lighting`: `mmode(MVIEWING=2)`; `deflight()`.
8. If `rgb_mode && !is_ci_only`: `RGBmode()`; then if `!is_indigo`:
   `blendfunction(BF_SA=4, BF_MSA=5)`, i.e. `dst = src·αs + dst·(1−αs)`.
9. `subpixel(TRUE)`; `gconfig()`.
10. Queue `REDRAW` (0x210), plus `WINFREEZE` (0x21b) and `WINTHAW` (0x21c) when
    not a saver.
11. `getplanes()`: 4 → `ncolors=15, basecolor=1`; 8 → `ncolors=128,
    basecolor=128`; otherwise `ncolors=128, basecolor=640`. Colormap mode only.
12. `savecolors(); defpanel();` then, only if `!is_saver`, `load_spectrum_map()`
    (colormap content; harmless in RGB).
13. `shademodel(FLAT=0)`, `concave(FALSE)`, `glcompat(GLC_OLDPOLYGON, 0)`.
14. `perspective(300, 1.0, 0.01, 1.0e6)` (fovy = 30.0°; near `0x3c23d70a`,
    far `0x49742400`).
15. If `has_lighting`: `loadident()` (loadmatrix identity into ModelView).
16. `polarview(10.0, 0, 0, 0)`. With all angles 0 this premultiplies
    ModelView by `translate(0, 0, −10)`. ModelView is then `T(0,0,−10)` for the
    rest of the run, because every drawing function pushes and pops around its
    own changes.
17. `lsetdepth(0, 0x7fffff)`.
18. `c4f(bkgRGBA)` (CI: `color(bkg)`); `clear(); swapbuffers(); clear();`
19. `n = 0; t = 0.0; gchan = 0; gchant = 0.0; gcol = (ncolors/2) + 640`
    (float); `bhue = 0.5; ghue = 0.5`.

### Reshape and fullscreen

* On `REDRAW` (handled every frame, §3 step 12): `reshapeviewport()`; then
  `perspective(300, aspect, 0.01, 10000.0)` (far `0x461c4000`), where
  `aspect = f32(XPMAX)/f32(YPMAX)` (single-precision divide of
  `getgdesc(GD_XPMAX)` by `getgdesc(GD_YPMAX)`) if `on(fullbutton)`, else
  `1.0`. Then `loadident()` (if `has_lighting`), `polarview(10,0,0,0)`,
  `lsetdepth(0,0x7fffff)`, `c4f(bkgRGBA)`, `clear()`. IrisGL delivers a REDRAW
  when the window maps, so far = 10000 in practice.
  **When it arrives depends on the window system, not on ep.** In `ep200`,
  `ep_long` and `ep_q` the main loop handles it on tick 1 (trace frame 3).
  In `ep_v_zl`, `ep_v_smear` and `ep_v_rzl-q`, libpanel's `pnl_dopanel`
  dequeues it first and puts it back (`qenter(20001, 0)`, `qenter(528, 1)`),
  so the main loop sees it on tick 2 (frame 4). In `-S` there is none. The
  port therefore treats REDRAW as an input event: by default it delivers one
  on tick 1 in windowed mode, and the trace comparison replays it in the
  tick where the trace has it (the main loop's `qread` returning 528,
  followed by `reshapeviewport`).
* `fullscreen` @0x40a574 (default script turns `full` on): window becomes
  `(0..XPMAX−1, 0..YPMAX−1)`, `perspective(300, XPMAX/YPMAX, 0.01, 10000)`,
  `loadident`, `polarview(10,0,0,0)`, `lsetdepth`, then
  `c4f(bkgRGBA); clear; swap; clear; swap`. If `stop` is on it does nothing
  except reset the button.
* `regularscreen` @0x40a904: restores the saved window, aspect 1.0, the same
  setup, `keepaspect(1,1)`, then clear/swap twice.
* **Aspect quirk:** `-S` without fullscreen keeps aspect 1.0 whatever the
  window shape.

### Lighting definitions (`deflight` @0x40e798, executed once)

`lmdef(DEFMATERIAL, 1, 15, @0x100000fc)`:

| token | values |
|---|---|
| AMBIENT (2) | 0.2 0.2 0.2 (`3e4ccccd`) |
| DIFFUSE (3) | 0.8 0.8 0.8 (`3f4ccccd`) |
| SPECULAR (4) | 1 1 1 |
| SHININESS (5) | 80 (`42a00000`) |
| LMNULL | |

`lmdef(DEFLIGHT, i, 10, …)` for i = 1..7. Every light has `LCOLOR (101) 1 1 1`
and `POSITION (102) x y z w`:

| light | addr | POSITION |
|---|---|---|
| 1 | 0x10000138 | ( 1,  0,  0.5, 0) |
| 2 | 0x10000160 | (-1,  0, -0.5, 0) |
| 3 | 0x10000188 | ( 1,  0, -0.5, 0) |
| 4 | 0x100001b0 | (-1,  0,  0.5, 0) |
| **5** | 0x100001d8 | ( 0,  1,  0.5, 0) |
| **6** | 0x10000200 | ( 0, -1, -0.5, 0) |
| 7 | 0x10000228 | ( 0,  1, -0.5, 0) |

`lmdef(DEFLMODEL, 1, 10, @0x10000250)`: `AMBIENT 1 1 1`, `LOCALVIEWER (201) 0`,
`ATTENUATION (202) 1 1`, LMNULL. Attenuation does not apply to infinite lights.
No TWOSIDE.

Only lights 5 and 6 are ever bound (see `uselight`).

---

## 3. Per-frame main loop (`main` body, 0x40f8ec..0x410f5c)

One loop iteration is one simulation step. Order:

1. `pnl_dopanel()`. On the very first iteration, if `rflag`: `fullscreen()`
   and start reading `panel.script`.
2. **Stop.** If `on(stopbutton)`: if `!on(smearbutton)`, `c4f(bkgRGBA)` and
   `clear()`. Then `drawit(); swapbuffers();`, then loop
   `{ if (!pnl_dopanel()) sginap(0); swapbuffers(); } while on(stopbutton)`.
   Simulation state did not change since the last frame, so this redraw makes
   both buffers hold the same picture and the repeated swaps do not flicker.
3. If `aflag || on(autobutton)`: `tasteQueue(); setacttargets(); animateacts();`.
   Then always `animatesliders()`. These are the animation subsystem; they only
   change actuator values.
4. **Time step:**
   * `t = f32(t + S(speed))` (single add)
   * `stepped = (f64(t) >= 1.0)`
   * `n = trunc(f64(n) + floor(f64(t)))`
   * `t = f32(fmod(f64(t), 1.0))`
5. RGB only: `gchant = f32(fmod(f64(f32(gchant + f32(D(chanrateslider) ·
   S(speed)))), 1.0))`.
6. **Record the current head slot** `s = slot(n)` (overwritten every frame, so
   the head tracks the sliders continuously):
   * `x[s] = puck.x`, `y[s] = puck.y` (positionpuck `+0xa4 → +8 / +0xc`)
   * `dzoom[s] = S(zoom)`, `arm[s] = S(arm)`, `wrist[s] = S(wrist)`,
     `dtwist[s] = S(twist)`, `size[s] = S(size)`
   * `dwheel = f32(S(speed) · S(wheel))`
   * `nlimit = trunc(S(nslider))`
   * `outline[s] = on(outlinebutton)`, `fatline[s] = on(fatlinebutton)`,
     `fill[s] = on(fillbutton)` (ints 0/1)
   * CI: `maprange[s] = S(maprange)`; `gcol += S(speed)·S(maprate)`
   * RGB, using **gchan from the previous frame**:
     `alpha[s] = D(adial[gchan])`, `alphaout[s] = D(alphaoutdial)`,
     `shiny[s] = S(shiny)`, `ambient[s] = S(ambient)`,
     `ghue = S(hslider[gchan])`, `glight = S(lslider[gchan])`,
     `bhue = f32(f64(bhue) + f64(f32(S(speed)·D(huebackratedial))) · 0.05)`
     (0.05 is a double at 0x10000e48, `3fa999999999999a`)
   * `textureindex[s] = currenttextureindex`, `dtextures[s] = S(s)`,
     `dtexturet[s] = S(t)`, `texturesmax[s] = S(S)`, `texturetmax[s] = S(T)`
7. Wrap. CI: wrap `gcol` into `[basecolor, basecolor+ncolors)` by repeated
   ±`ncolors` (single precision). RGB: `while ghue < 0: ghue += 1`;
   `while ghue >= 1: ghue −= 1`; the same for `bhue` (single precision).
8. RGB: `hue[s] = ghue`, `light[s] = glight`. **Channel select:** if the panel's
   current actuator `pnl_ca` is `activebutton[i]`, then `gchan = i`. Otherwise:
   `acc = 0; gchan = 0; for i in 0..4: acc = f32(acc + D(dwellslider[i]));
   if gchant < acc: gchan = i; break`. If no channel matches, `gchan` stays 0.
   If `!on(activebutton[gchan])`, set every activebutton val to 0, set
   `activebutton[gchan]` to 1, and redraw them. Then a UI-only refresh:
   for each channel `mapcolor(0x300+i, irgb(HLS(fmod(S(hslider[i]),1),
   S(lslider[i]), 1)))`.
9. **Background colour:** if `on(backgroundbutton)`:
   `bkg = trunc(gcol) + ncolors/2`, minus `ncolors` if
   `≥ basecolor+ncolors`. RGB: `bkgRGBA.rgb = HLS(bhue, 0.5, 1.0)` (full
   saturation, mid lightness). Otherwise `bkg = 0` and `bkgRGBA.rgb = (0,0,0)`.
   RGB always: `bkgRGBA.a = D(alphabackdial)`. The framebuffer alpha does not
   affect the displayed image, because blending uses source alpha only.
10. If `stepped`: `dflip = S(flip)`, `dspin = S(spin)`;
    `gflip = f32(fmod(f64(f32(gflip+dflip)), 360))`; `flip[s] = gflip`; the
    same for `gspin`/`spin[s]`. Flip and spin are written **only on the frame
    that opens a new slot**, so the head slot keeps its value while `t < 1`.
    `fmod` keeps the sign, so values lie in (−360, 360).
11. `pnl_userwinevent`: REDRAW → reshape (§2). WINFREEZE/WINTHAW toggle a
    "frozen" flag. While frozen, each iteration blocks in `qread` and re-queues
    the event.
12. `wheel = f32(fmod(f64(f32(wheel − dwheel)), 360.0))`.
13. **Buffer handling:**
    * `!on(smearbutton)`: `c4f(bkgRGBA)` (CI `color(bkg)`); `clear()`.
    * `on(smearbutton)`:
      * if `on(fadebutton)`: `fadebackground(bkg, 1)`
      * if `on(smoothbutton)`: `drawit(); swapbuffers();`; then if
        `on(fadebutton)`: `fadebackground(bkg, 0)`; then `pnl_drawpanel()`
14. `drawit(); swapbuffers();`

There is no other frame pacing. The rate is whatever `swapbuffers` (vertical
retrace) allows. In smooth mode each step has two swaps.

---

## 4. Interpolation helpers

`twixt(k, A, t)` @0x410fdc. `p = k−1` (if `p < 0`, `p += 128`), `tf = f32(t)`:

```
result = f32( f64(A[p]) * (1.0 − f64(tf))  +  f64(f32(A[k] * tf)) )
```

The first product is double; the second is single-precision.

`foldtwixt(k, A, t, per)` @0x411050. `P = f32(per)`, `c = A[k]`, `q = A[p]`,
`d = f32(c − q)`:

* if `f64(P)*0.5 < f64(d)`: `f32( f64(f32(q+P))·(1−tf) + f64(f32(c·tf)) )`
* else if `f64(d) < f64(−P)*0.5`: `f32( f64(q)·(1−tf) + f64(f32(f32(c+P)·tf)) )`
* else: the same as `twixt`.

The result is **not** re-wrapped. Hue (period 1) can come out in about
[−0.5, 1.5]; in practice, with recorded hues in [0,1), it stays in [0, 1.5).
Flip and spin use period 360.0, light uses period 1.0. Folding the light value
is the original's behaviour.

Interpolation is always between history slot `k−1` (weight `1−t`) and `k`
(weight `t`). Slot `n` therefore glides from the previous sample toward the
live slider values, and the strand moves smoothly as `n` increments.

---

## 5. `drawit` @0x411614

```
if on(ribbonbutton): drawribbons(); return           # §6 (no uselight!)
if on(zbufbutton) && has_zbuffer: zclear()
fcol = gcol                                          # CI running index
if on(lightbutton): uselight()                       # binds lights under ModelView = T(0,0,-10)
pushmatrix()
rotate(trunc(f64(wheel)*10.0) as short, 'x')

# centring pre-pass, newest to oldest
for k = n down to n-nlimit+1:
    translate(0, 0, f32(f64(-twixt(slot(k), dzoom, t)) * 0.5))

curtex = 0
for k = n down to n-nlimit+1:                        # newest drawn first
    p = slot(k)
    if textureindex[p] != curtex: simplesettexture(textureindex[p]); cpack(0xffffffff); curtex = textureindex[p]
    X=twixt(x) Y=twixt(y) Z=twixt(dzoom) ARM=twixt(arm) WR=twixt(wrist) SZ=twixt(size)
    SPIN=foldtwixt(spin,360) FLIP=foldtwixt(flip,360) TW=twixt(dtwist)
    H=foldtwixt(hue,1) A=twixt(alpha) L=foldtwixt(light,1) AO=twixt(alphaout)
    AMB=twixt(ambient) SH=twixt(shiny)
    # texture-scroll bug: for ALL 128 entries i: textures[i] += dtextures[i]*S(speed);
    #                     texturet[i] += dtexturet[i]*S(speed)   (single precision)
    TS = twixt(textures)          # twixt(texturet) is computed but discarded
    TMAX = twixt(texturetmax); SMAX = twixt(texturesmax)

    RGB:  colRGBA.rgb = HLS(H, L, 1.0)
          H2 = f32(f64(H) + 0.5); if 1.0 < f64(H2): H2 = f32(f64(H2) − 1.0)
          colRGBA.a = A; outlinecolRGBA.a = AO
          outlinecolRGBA.rgb = HLS(H2, 1.0 − f64(L), 1.0)     # complementary hue, inverted lightness
    CI:   if outline[p]: oc = trunc(fcol) + ncolors/2, minus ncolors if ≥ basecolor+ncolors
          fcol = fcol − maprange[p], wrapped into [basecolor, basecolor+ncolors)
          (fill uses trunc(fcol) after the subtraction; the outline index used fcol before it)

    if !on(lightbutton): RGB c4f(colRGBA) | CI color(trunc(fcol))
    else:                ep_lmcolor(colRGBA, AMB, SH)
    linewidth(fatline[p] ? 3 : 1)

    COPY(M):  pushmatrix()
              translate(X, Y, 0)
              rotate(rs = trunc(f64(SPIN)*10) as short, 'z')
              translate(0, ARM, 0)
              rotate(rf = trunc(f64(FLIP)*10) as short, 'y')
              translate(WR, 0, 0)
              scale(SZ, SZ, 1.0)
              if has_lighting && on(lightbutton): n3f(ep_trinormal)
              if fill[p]:    drawshape(p, FILL, TS, TS, SMAX, TMAX)
              if outline[p]: set outline colour (c4f(outlinecolRGBA) | CI color(oc) | ep_lmcolor(outlinecolRGBA,AMB,SH) when lit)
                             drawshape(p, OUTLINE, ...)
                             restore fill colour (c4f(colRGBA) | color | ep_lmcolor(colRGBA,AMB,SH))
              popmatrix()

    COPY()                               # copy 1
    if sflag == 0:                       # quad-strand mode (-q)
        scale(1,-1,1);   COPY()          # copy 2: base·Sy
        rotate(1800,'z'); COPY()         # copy 3: base·Sy·R180 = base·Sx
        scale(1,-1,1);   COPY()          # copy 4: base·R180
        # net effect: the base matrix is now base·R180 (it alternates every slot)
    rotate(trunc(f64(TW)*10) as short, 'z')
    translate(0, 0, Z)
popmatrix()
if curtex != 0: simplesettexture(0)
```

Notes on the transform chain:

* IrisGL `rotate/translate/scale` premultiply in IrisGL's row-vector
  convention. That is the same composition order as OpenGL's
  `glRotate/glTranslate/glScale` with column vectors: the last call applies
  to the vertex first. A shape vertex `v` is transformed as
  `Proj · T(0,0,−10) · Rx(wheel) · Π_pre T(0,0,−Z_j/2) · Π_{older slots}[Rz(TW_j)·T(0,0,Z_j)] ·
  [mirror] · T(X,Y,0) · Rz(SPIN) · T(0,ARM,0) · Ry(FLIP) · T(WR,0,0) · S(SZ,SZ,1) · v`.
  Angles are integer tenths of a degree, so the matrices use
  `sin/cos(angle_int · π/1800)`.
* The newest slot (k = n) is drawn first, at the deepest point of the
  pre-pass (−½·Σ Z). Each older slot sits one `Rz(TW)·T(0,0,Z)` further along.
  The strand has `nlimit` elements, centred on the origin along its own axis.
* `translate(0,0,Z)` uses the **unhalved** `Z`. The pre-pass sums `−Z/2` over
  the same `nlimit` slots, but the drawn strand uses `Rz(TW)` between steps, so
  the two only cancel along z. Implement it literally.
* History slots older than the frame count are calloc zeros (size 0, alpha 0),
  so they are invisible. The strand "grows in" over the first `nlimit` frames
  (`speed = 1` gives one slot per frame).
* Mirrors (`scale(1,−1,1)`) reverse winding. That is harmless because there is
  no backface culling and lighting is not two-sided.
* In quad mode the order of the four copies alternates between consecutive
  slots (base·R180 accumulates). That changes blend order, so emulate it
  literally.

### 5.1 `drawshape(p, mode, a, b, smax, tmax)` @0x411194

* `mode == 0` (outline): `bgnclosedline()`, the vertices, `endclosedline()`.
* `mode == 1` (fill): `bgnpolygon()`, the vertices, `endpolygon()`.
* Vertex list. If `textureindex[p] == 0`: if `on(trianglebutton)`, the
  `triangle[0..2]`, else the `square[0..3]`, all `v2f` (z = 0). If
  `textureindex[p] != 0`: always the square. In fill mode each vertex is
  preceded by `t2f`: `(b,a)`, `(b, a+tmax)`, `(b+smax, a+tmax)`, `(b+smax, a)`
  for `square[0..3]`. drawit passes `a = b = twixt(textures)`, so the "t"
  coordinate also uses the s value. That is a bug in the original; keep it.
  The texcoord floats are staged at 0x1000031c..0x338.

### 5.2 `uselight` @0x40e8f0

If `has_lighting`: `lmbind(LIGHT5=1105, 5)`, then if `!is_indigo`:
`lmbind(MATERIAL=1000, 1); lmbind(LIGHT6=1106, 6); lmbind(LMODEL=1200, 1)`.
Light positions are transformed by the ModelView at bind time, which is
`T(0,0,−10)`. For w = 0 lights only the direction matters, so in eye space:
L5 = dir(0, 1, 0.5) and L6 = dir(0, −1, −0.5). The two are exactly opposite, so
for any normal at most one of them gives diffuse light: in effect
`|N·L|`-style two-sided diffuse.

### 5.3 `ep_lmcolor(rgba, amb, shiny)` @0x40e99c

If `has_lighting`, it rewrites the 17-float material array at 0x10000278 and
then calls `lmdef(DEFMATERIAL, 1, 17, arr); lmbind(MATERIAL, 1)`:

| token | value |
|---|---|
| AMBIENT | `rgba[0..2] · f32(amb)` (single multiply) |
| DIFFUSE | `rgba[0..2]` |
| SPECULAR | 1, 1, 1 (constant from .data 0x1000029c) |
| SHININESS | `f32(floor(f64(f32(shiny)) · 128.0))` (0..128) |
| ALPHA | `rgba[3]` |

EMISSION stays at its default of (0,0,0) because material 1 was first defined
without it.

### 5.4 Lighting result (IrisGL `lmdef` model)

Applies once lighting is enabled (MATERIAL ≠ 0 and LMODEL ≠ 0 bound) and
a normal was the last colour/normal command before the vertex (`lmcolor` is
the default LMC_COLOR):

```
N = normalize(NormalMatrix · (0,0,1))     # NAUTO: renormalised because S(size,size,1) is not orthonormal
c = emission(0) + lmodelAmbient(1,1,1)·matAmb (= rgb·amb)
for L in {L5, L6} (unit eye-space directions):
    c += lightAmb(0)·matAmb
    d = N·L
    if d > 0:
        c += d · Lcolor(1) · rgb
        if shininess > 0:  c += max(0, N·H)^shininess · (1,1,1),  H = normalize(L + (0,0,1))   # infinite viewer
color = min(c, 1.0) per channel (no lower clamp); alpha = rgba[3]
```

**UNCERTAIN:** (a) whether IrisGL normalises the w = 0 light direction (this
spec assumes it does, as OpenGL does), and (b) whether specular is gated on
`N·L > 0`. A pixel-read trace of a lit square at known flip angles on real
hardware would settle both. The man page states that **shininess 0 disables
specular**; this differs from OpenGL, where `x^0 = 1`.

Lines (outlines) are lit with the same normal.

### 5.5 Colour-command vs normal precedence

LMC_COLOR semantics from the `lmcolor` man page: if a colour command (`c4f`,
`cpack`, `color`) was the last of {colour, normal} sent before a vertex, the
vertex takes that flat colour; if a normal was last, the vertex is lit. ep never
unbinds the material (no `lmbind(MATERIAL,0)` anywhere), so once the light
button has been used, lighting stays "enabled". Turning the light button off
still gives unlit colours, because drawit then issues `c4f` and no `n3f`.

---

## 6. `drawribbons` @0x41364c (ribbon button)

Returns at once unless `has_tmesh`. Otherwise:

```
if on(zbufbutton) && has_zbuffer: zclear()
pushmatrix(); rotate(trunc(f64(wheel)*10),'x')
pre-pass translate(0,0,-Z/2) over the strand (as in drawit)
ncopies = sflag ? 1 : 4
for c in 0..ncopies-1:
    fcol = gcol
    pushmatrix(); bgntmesh()
    for k = n down to n-nlimit+1:
        p = slot(k); X,Y,Z,ARM,WR,SZ,SPIN,FLIP,TW,H,A,L,AO as in drawit (no ambient/shiny/texture twixts)
        RGB: colRGBA/outlinecolRGBA computed exactly as in drawit (the outline colour is unused)
        CI:  fcol -= maprange[p] and wrap
        if !has_lighting or !on(lightbutton): c4f(colRGBA) | CI color(trunc(fcol))
        else: ep_lmcolor(colRGBA, <uninitialised stack float @sp+0x9c>, <uninitialised @sp+0x98>)
        linewidth(fatline[p] ? 3 : 1)          # no effect on a tmesh
        pushmatrix()
        translate(X,Y,0); rotate(SPIN·10,'z'); translate(0,ARM,0); rotate(FLIP·10,'y'); translate(WR,0,0); scale(SZ,SZ,1)
        v2f(triangle[0])                        # (0, 0, 0)
        v3f(&triangle[1])                       # reads 0.2, 0.0, 0.1  →  vertex (0.2, 0, 0.1)
        popmatrix()
        rotate(trunc(f64(TW)*(c<2 ? 10.0 : -10.0)),'z')
        translate(0,0,Z)
    popmatrix()
    if c even: rotate(1800,'z') else scale(1,-1,1)   # copies: B, B·R180, B·Sx (−twist), B·Sy (−twist)
endtmesh(); popmatrix()
```

Ribbon facts:

* Each slot contributes two vertices, `A = (0,0,0)` and `B = (0.2, 0, 0.1)`.
  `v3f` is pointed at `triangle+8`, so z comes from `triangle[2].x`. That is a
  bug in the original; keep it. The vertex order is
  `A_n, B_n, A_{n−1}, B_{n−1}, …`, so the triangles are (A_n,B_n,A_{n−1}),
  (A_{n−1},B_n,B_{n−1}), and so on (IrisGL tmesh replaces the older vertex).
* With `shademodel(FLAT)` each triangle takes the colour of its **third
  (newest-emitted) vertex**. Every quad between slot k+1 and slot k is
  therefore coloured with slot k's colour, and slot n's own colour never shows.
* `fill`, `outline`, textures and `n3f` are not used. `uselight` is **not**
  called in ribbon mode.
* **UNCERTAIN (bug):** in quad mode `bgntmesh` is called four times with only
  one `endtmesh`. Treat each `bgntmesh` as starting a new strip (the most
  likely hardware behaviour). A trace or a capture from real hardware would
  confirm it.
* **Lit ribbons (bug; trace-checked in §0.1).** With the light button on,
  `ep_lmcolor` gets garbage ambient and shininess (the stack slots
  `0x98`/`0x9c` are never written in drawribbons; the IRIX 6.5 build has the
  same source bug). In traces the ambient is ≈2.5e−29, occasionally ≈3.7e+19
  for whole frames, and the shininess is 0. Ribbon mode never calls
  `uselight` and never sends a normal. Port rule, with the current colour `C`
  being the last `c4f`/`cpack`, which is `bkgRGBA` when smear is off:
  * If LMODEL has never been bound (`uselight` never ran this session),
    lighting is disabled and ribbon vertices take `C`. This is the
    trace-confirmed case.
  * If lighting was enabled earlier (light was on in a non-ribbon frame), a
    vertex is lit only if the last `n3f` is more recent than the last colour
    command (§5.5). With smear off, `c4f(bkgRGBA)` always comes later, so the
    vertex again takes `C`. With smear on, the vertices are lit with the
    normal from the last square drawn, ambient ≈ 0 and no specular.
    **UNCERTAIN:** that last case is reasoned from the man pages and not
    rendered on hardware.

---

## 7. Colour conversion

### 7.1 `hls_to_rgb(h, l, s, &r, &g, &b)` @0x416a7c (arguments are doubles)

```
lf = f32(l); sf = f32(s)
m2 = (f64(lf) <= 0.5) ? f32(f32(1.0 + sf) * lf) : f32(f32(lf + sf) − f32(lf*sf))
if sf == 0: r = g = b = lf
else:
   m1 = f32(f32(2.0*lf) − m2)
   hd = f32(f64(f32(h)) * 360.0)                    # double multiply, 360.0 from 0x10000f30
   r = value(m1, m2, hd + 120.0); g = value(m1, m2, hd); b = value(m1, m2, hd − 120.0)
```

`value(n1, n2, hue)` (FUN_00416964 @0x416964), `x = f32(hue)`:

```
if 360.0f < x: x −= 360.0f
if x < 0:      x += 360.0f
if x <  60: return f32(f64(n1) + f64(f32(n2−n1)) * (f64(x)/60.0))
if x < 180: return n2
if x < 240: return f32(f64(n1) + f64(f32(n2−n1)) * (f64(f32(240.0f−x))/60.0))
return n1
```

Hue 0 is red, 1/3 green, 2/3 blue. Only one ±360 wrap is applied, which is
enough for inputs in roughly (−1/3, 4/3).

### 7.2 Other converters

* `rgb_to_irgb` @0x4170a4: `i = trunc(f64(f32(c)) * 255.0)` (truncation; 255.0
  is a double at 0x10000f88). Used only for `mapcolor` (colormap and panel
  swatches).
* `hsv_to_rgb`, `yiq_to_rgb`, `cmy_to_rgb`, `irgb_to_rgb`, `rgb_to_*`,
  `torgb/fromrgb/setcolorsys`: **not reachable** from the render path. No
  callers, or only callers that are themselves dead.

### 7.3 Colormap (CI) loaders

* `load_spectrum_map` @0x408ef4: for i in [0, ncolors):
  `mapcolor(basecolor+i, irgb(HLS(f32(i)/f32(ncolors), 0.5, 1.0)))`. Called
  only when not in saver mode.
* `load_contour_map` @0x409068 (**dead**: no callers): for i in [0, ncolors]
  (inclusive), `u = f32(i)/f32(ncolors)`,
  `fr = f32(fmod(u, 0.10638297872340426))` (= 5/47, double at 0x10000d20),
  `HLS(f32(u−fr), fr*6.5 + 0.1, 1.0)`.
* `load_band_map` @0x40925c (**dead**): spectrum, then black for
  `i ∈ [20, ncolors−20)` where `i % 20 < 5`.
* In CI mode the fill index per slot is `trunc(fcol)`, where `fcol` starts at
  `gcol` and drops by `maprange[slot]` per slot. The outline index is
  `trunc(fcol_before) + ncolors/2` (wrapped). The background index is
  `trunc(gcol) + ncolors/2` when the background button is on, else 0.

---

## 8. Background, trails and buffers

| state | front/back writes | per-step behaviour |
|---|---|---|
| smear off (default) | `frontbuffer(0) backbuffer(1)` | `c4f(bkgRGBA); clear()` then draw, swap |
| smear on, smooth off | `frontbuffer(1) backbuffer(1)` (draw into both) | no clear; optional fade; draw; swap |
| smear on, smooth on | `frontbuffer(0) backbuffer(1)` | `[fade(bkg,1)]`; draw; swap; `[fade(bkg,0)]`; panel; draw; swap. The same state is drawn into both buffers. |

* `setsmear(btn)` @0x40a2b8. Turning on with smooth off and stop off:
  `c4f(bkgRGBA); clear(); drawit();` then `frontbuffer(1); backbuffer(1)`.
  With smooth off and stop on, only `frontbuffer(1); backbuffer(1)`.
  Turning on with smooth on: no buffer change. Turning off:
  `frontbuffer(0); backbuffer(1)`.
* `setsmooth(btn)` @0x40a1a8: only acts while smear is on. Smooth on gives
  `frontbuffer(0) backbuffer(1)`; off gives `frontbuffer(1) backbuffer(1)`.
* `setzbuf(btn)` @0x40a494: on gives `zbuffer(1)` and `zclear()` (if
  `has_zbuffer`); off gives `zbuffer(0)`. The default is off. When on, drawit
  and drawribbons call `zclear()` at the start of **every** drawit, even while
  smearing. Depth test is IrisGL's default `ZF_LEQUAL`, with depth writes on.
* `ep_clear` @0x408ec4 is just `clear()`. It fills the viewport with the
  current colour, honours the polygon pattern and writemask, and **ignores
  blending and z**.
* `togglechanmode` @0x409764 only shows or hides panel widgets. It has no
  rendering effect.

### 8.1 `fadebackground(idx, step)` @0x416774 (stippled dissolve)

```
phase = (phase + step); while phase ≥ 16: phase −= 16        # static int @0x1000abc8, starts 0
pat[0..15] = 0                                                # 16 shorts @0x1000aba0 ("tex")
r = ROW[phase]; sh = SHIFT[phase]
w = (0x1111 << sh) & 0xffff
pat[r] = pat[r+4] = pat[r+8] = pat[r+12] = w
defpattern(2, 16, pat); color(idx); setpattern(2); clear(); setpattern(0)
```

| phase | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| SHIFT @0x10000340 | 0 | 2 | 2 | 0 | 1 | 3 | 3 | 1 | 0 | 2 | 2 | 0 | 1 | 3 | 3 | 1 |
| ROW @0x10000380 | 0 | 2 | 0 | 2 | 1 | 3 | 1 | 3 | 1 | 3 | 1 | 3 | 0 | 2 | 0 | 2 |

Pattern semantics (`defpattern` man page): word index is the row from the
bottom, bit 15 is the leftmost pixel, and the pattern is tiled from the
**screen's** bottom-left corner. Each call overwrites exactly one pixel in every
4×4 block: the pixels with `(y_screen mod 4) == ROW[phase]` and
`(x_screen mod 4) == 3 − SHIFT[phase]`. The 16 phases cover all 16 positions
in an ordered-dither sequence, so after 16 steps every trail pixel has been
replaced. Older paint therefore dissolves away over 16 steps with no blending.
Screen origin vs window origin only matters in windowed mode; fullscreen makes
them the same.

**Fade colour (resolved by an Indy screen grab).** The trace confirms that
`color(0)` is issued in RGB mode on every fade (§0.1). `fadebackground` calls
`color(idx)` (a colour-index command; `idx = bkg` is 0 when the background
button is off, otherwise a CI index); the `color` man page says calling it in
RGB mode is an error. On the Indy the call is not ignored: the index replaces
the low 16 bits of the packed colour, so red becomes the index's low byte and
green its high byte, while blue and alpha keep the last `c4f` values. With the
background off the trail therefore dissolves towards pure blues. A port that
ignores the call (keeping the whole current colour) matches only 68% of a
smear+fade frame within one 4-bit step; with this rule it matches 97.7%
(docs/ground-truth.md). Other SGI graphics boards may differ; ep-core keeps
"ignore" and "colour map" as options.

---

## 9. Floating-point and integer details that affect exactness

* Toggle tests come in two forms. Most call sites test "on" as
  `mid < val`; the colour and buffer branches (`drawit`'s
  `c4f`/`ep_lmcolor` choice, `drawribbons`, main's smear test, `setsmear`'s
  smooth and stop tests, `setzbuf`, `setauto`, the active-button check) test
  "off" as `val < mid`. They differ only for a value exactly at the midpoint
  (0.5 for a 0/1 toggle, reachable with `actset`): there the light button,
  for example, makes drawit use `ep_lmcolor` without `uselight` or `n3f`.
  The port keeps each call site's form.
* All float→int conversions truncate toward zero (FCSR RM forced to RZ).
  This covers rotate angles (`(short)trunc(f64(a)*10.0)`), `n`, `nlimit`,
  `bkg` and colour indices, and `rgb_to_irgb`.
* `floor` is only used for `n` (with `t`) and for the shininess value.
* `fmod` (C semantics, sign of the dividend) is used for `t` (1.0), `gchant`
  (1.0), `gflip`/`gspin`/`wheel` (360.0) and the channel-swatch hue.
* Hue and bhue wrap through repeated single-precision ±1.0 loops.
* Constants:

  | constant | value | location | encoding |
  |---|---|---|---|
  | rotate scale | 10.0 | `lui 0x4024` | double |
  | negated twist | −10.0 | `0xc024` | double |
  | half | 0.5 | `0x3fe0` | double |
  | one | 1.0 | `0x3ff0` | double |
  | hue offset | 120.0 | `0x405e` | double |
  | HLS ramp divisor | 60.0 | `0x404e` | double |
  | HLS wrap and branch | 360/60/180/240 | | single (`43b40000`, `42700000`, `43340000`, `43700000`) |
  | angle wraps | 360.0 | 0x10000e50–0x10000e6c | doubles |
  | shininess scale | 128.0 | `0x4060` | double |
  | bhue rate | 0.05 | 0x10000e48 | double |
  | irgb scale | 255.0 | 0x10000f88 | double |
  | perspective | fovy 300, near 0.01f, far 1e6f / 10000f | | |
  | polarview distance | 10.0f | `41200000` | single |
  | depth range | 0..0x7fffff | | |

* History arrays and `twixt` are single precision, with the double/single mix
  given in §4.
* No random numbers are used in rendering (drand48 only drives animation
  targets).

---

## 10. Implementation notes for Metal

**Matrix model.** Keep a ModelView stack (depth ≥ 32) and a separate
Projection matrix; `mmode(MVIEWING)` is always active.

* `perspective(fovy_tenths, aspect, n, f)`: standard
  `gluPerspective(fovy/10°, aspect, n, f)`, with Metal z mapped into [0,1].
* `polarview(d,0,0,0)` = `translate(0,0,−d)`.
* `rotate(a_tenths, axis)` = rotation by `a/10` degrees, right-handed, post-
  multiplied in OpenGL order.
* `translate`, `scale`, `pushmatrix`, `popmatrix`, and `loadmatrix(I)` for
  `loadident`.
* Normal matrix = inverse-transpose of the ModelView 3×3, renormalised when the
  matrix is not orthonormal (NAUTO).

**Rasterisation.**

* `subpixel(TRUE)`: no vertex snapping. Polygons are sampled at pixel centres;
  lines are half-open.
* `shademodel(FLAT)`: polygons take the colour of their last vertex. A tmesh
  triangle takes the colour of its third (newest) vertex. Use a `[[flat]]`
  attribute with a provoking-vertex rule that matches.
* `concave(FALSE)`: all shapes are convex, so a triangle fan is enough.
* `linewidth(1|3)`: aliased wide lines are `n` pixels across the **minor axis**
  (a vertical extent for x-major lines, horizontal for y-major). Expand them to
  quads in a shader; Metal has no wide lines.
* `bgnclosedline`: a line loop.

**Blending.** `blendfunction(BF_SA, BF_MSA)`:
`rgb = src.rgb·src.a + dst.rgb·(1−src.a)`. Alpha uses the same factors (not
visible). Use an 8-bit UNORM target (`.bgra8Unorm`, no sRGB, no dither) so the
repeated blending in smear mode quantises the way 8/8/8 hardware did. This
matches the target class in §0. Indy XL24 double-buffered RGB is 4/4/4 with
dither, and is deliberately not emulated. **UNCERTAIN:** SGI's exact rounding
of blend arithmetic and of `c4f` → 8 bits.

**Depth.** 24-bit or 32-bit float depth, test `LEQUAL`, writes on, enabled only
while the z-buf button is on. Clear depth to far at the start of each
`drawit`/`drawribbons` when enabled. Outlines drawn over fills at equal depth
rely on LEQUAL; a small depth bias may be needed to match.

**Buffers.** Model `frontbuffer`/`backbuffer` write enables explicitly. Keep
two persistent colour textures (front and back) and present by swapping.
"Draw into both" means rendering the same command list into both textures. Do
not use Metal's auto-cleared drawables: `clear()` is an explicit full-viewport
fill with no blending, no depth, and the current 16×16 pattern used as a
stencil mask (screen-aligned).

**Lighting.** Implement the §5.4 formula per vertex. Under flat shading only
the last vertex matters, and with the constant normal per shape it equals a
per-primitive value. Use two infinite lights, infinite viewer, lmodel ambient
(1,1,1), emission 0, specular (1,1,1), shininess `floor(shiny·128)` with
0 → no specular, clamp ≤ 1 only, and alpha from the material. Track
"last command was colour or normal" per vertex (§5.5).

**Textures** (only with `-t`). `texdef2d` uses 4 components. For non-256²
images: `TX_MINFILTER = TX_MIPMAP_BILINEAR`, `TX_MAGFILTER = TX_BILINEAR`. For
256×256: `TX_MAGFILTER = TX_BILINEAR`, `TX_MINFILTER = TX_POINT`,
`TX_WRAP = TX_CLAMP` (props @0x100003d0 / 0x100003e4). `tevdef(1, TV_MODULATE)`
(@0x10000400). The colour is `cpack(0xffffffff)` before textured slots.

**IrisGL calls the renderer must support (with the arguments ep uses):**

| call | arguments |
|---|---|
| `winopen`, `prefposition`, `keepaspect` | (1,1) |
| `winconstraints`, `winposition`, `reshapeviewport` | |
| `foreground`, `noborder` | |
| `doublebuffer`, `RGBmode`, `gconfig`, `getgdesc`, `getplanes` | getgdesc items 0, 1, 9 |
| `mmode` | (MVIEWING) |
| `perspective` | (300, aspect, 0.01, 1e6 or 10000) |
| `polarview` | (10.0, 0, 0, 0) |
| `loadmatrix` | (identity) |
| `lsetdepth` | (0, 0x7fffff) |
| `pushmatrix`, `popmatrix` | |
| `rotate` | (short tenths, 'x' / 'y' / 'z') |
| `translate` | (f, f, f) |
| `scale` | (f, f, 1) and (1, −1, 1) |
| `subpixel` | (1) |
| `shademodel` | (FLAT) |
| `concave` | (0) |
| `glcompat` | (GLC_OLDPOLYGON, 0) |
| `blendfunction` | (BF_SA, BF_MSA) |
| `zbuffer` | (0 / 1) |
| `zclear` | |
| `clear` | |
| `frontbuffer` | (0 / 1) |
| `backbuffer` | (1) |
| `swapbuffers` | |
| `defpattern` | (2, 16, words) |
| `setpattern` | (2 / 0) |
| `c4f` | (rgba) |
| `cpack` | (0xffffffff) |
| `color` | (CI index) |
| `mapcolor` | (CI and panel swatches) |
| `linewidth` | (1 / 3) |
| `n3f` | ((0,0,1)) |
| `v2f` | |
| `v3f` | |
| `t2f` | |
| `bgnpolygon` / `endpolygon` | |
| `bgnclosedline` / `endclosedline` | |
| `bgntmesh` / `endtmesh` | |
| `lmdef` | (DEFMATERIAL 1 with AMBIENT/DIFFUSE/SPECULAR/SHININESS/ALPHA; DEFLIGHT 1..7 with LCOLOR/POSITION; DEFLMODEL 1 with AMBIENT/LOCALVIEWER/ATTENUATION) |
| `lmbind` | (MATERIAL, 1), (LIGHT5, 5), (LIGHT6, 6), (LMODEL, 1) |
| `texbind` / `tevbind` | (0, idx) |
| `texdef2d` / `tevdef` | |

---

## 11. Open questions (what would resolve each)

1. ~~**RGB-mode `color()` in `fadebackground`.**~~ Resolved by an Indy screen
   grab (§8.1): the index replaces the low 16 bits of the packed colour. Other
   SGI graphics boards may differ.
2. **Lit ribbons.** Mostly resolved (§0.1, §6). The garbage values are
   measured, and in a ribbon-only session lighting is disabled, so ribbons take
   the current colour (`bkgRGBA`). Still open: the smear-on case after
   lighting was enabled in square mode (needs a pixel capture).
3. **Four `bgntmesh` calls with one `endtmesh`** in quad ribbon mode. Settle
   with a capture of `-q` with ribbons.
4. Normalisation of w = 0 light directions, and whether specular is gated by
   N·L > 0. Settle with pixel reads of a lit quad.
5. Exact 8-bit quantisation and rounding in blending, `c4f`, and lighting
   (×255 and then truncate or round?). Settle with pixel reads of known
   colours and alphas.
6. Frame pacing: assumed one step per vertical retrace (60–76 Hz on period
   monitors). The rendering spec only fixes "one step per loop iteration".
7. Screen-aligned vs window-aligned pattern origin in windowed mode (matters
   only for the fade's phase pattern).
8. ~~No trace coverage of the lit, z-buffer, ribbon, smear and fade paths.~~
   Done (§0.1): the call sequences match. The open items left are only what
   calls cannot show: items 3, 4, 5 and the smear-on lit-ribbon case in §6.
   (Item 1 needed a framebuffer capture too, and has since had one.)
9. Quad-mode ribbons crashed Indy libgl's software tmesh rasteriser at
   frame 189 (`_alpha_span_zb` ← `__triang_al_zb` ← `_tmesh_doit0`). This may be
   related to the unbalanced `bgntmesh`. It cannot be emulated meaningfully;
   the port should draw each copy as its own strip.
