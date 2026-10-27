# electropaint (ep, IRIX 5.3): controls, modes and panels

Source: the Ghidra export of `/usr/demos/bin/ep` (IRIX 5.3, o32 big-endian) at
`re/ghidra/ep-irix53/`, cross-checked against the IRIX 6.5 N32 build at `re/ghidra/ep-irix65/`.
Addresses are function bodies (the `*_gpsetup` stub plus 0xc). The Ghidra decompile drops the
varargs and double arguments of `mkepslider` and several `ep_mkact()` calls, so those values
were recovered from the machine code with two helpers written for this work:

* `re/tools/mipsdis.py EXPORTDIR FUNC` prints an annotated disassembly (GOT symbols, constants,
  float/double operands).
* `re/tools/mipscalls.py EXPORTDIR FUNC [CALLEE_REGEX]` is a linear-pass emulator that prints the
  register and stack arguments of every call. Watch for branch-delay slots: values that differ
  between `if` arms must be checked by hand (this matters for `rat`/`spn`/`flp`; see §3.3).

The numbers below come from the code and the initialised `.data`. Label strings and ranges
are given verbatim because the port's UI needs them. Panel coordinates are libpanel
"panel units": y points up, and on screen 1 unit is 40 px (`panel+0x50 = 40.0` in every ep
panel; libpanel's default is 50).

---

## 1. Build-time configuration

The capability flags live in initialised `.data` (`rflag` @ 0x10000000 … `has_zbuffer` @
0x1000003c). Most of them never change at run time:

| flag | initial | changed by | effect on the UI |
|---|---|---|---|
| `rgb_mode` | 1 | never | the "colors" panel exists; the "fade (W)" button is never added; the `rat`/`rng` sliders are never built |
| `has_lighting` | 1 | never | `amb`/`shn` sliders and the "light (L)" button exist |
| `has_tmesh` | 1 | never | "ribbons (R)" button exists |
| `has_textures` | 1 | never | the "texture" panel always exists (`-t` only adds image buttons) |
| `has_zbuffer` | 1 | `main` clears it if `is_indigo`/`is_ci_only` | "z-buf (Z)" button exists |
| `is_indigo`, `is_ci_only` | 0 | `main`: `getgdesc(GD_BITS_NORM_DBL_RED)` returns 0 → ci_only; less than 4 → indigo | z-buffer off on these machines |
| `aflag`, `bflag` | **1, 1** | options | the internal ("b") script runs by default |
| `cflag` | 0 | `-c` | panels hidden unless `-c` |
| `sflag`/`qflag` | 1/0 | `-s`/`-q` | single/quad strand |

The port can treat the build as **RGB, lit, tmesh, textured, z-buffered**. The colour-index
variants (the `rat` and `rng` sliders, "fade (W)", and the 4-plane colour remap in `main`) are
dead code in this binary. They are documented in §3.3 only for completeness.

---

## 2. libpanel actuator model (what the port must emulate)

### 2.1 Actuator fields used by ep (0xc4-byte struct, `pnl_mkact` @ 0x418808)

| off | meaning | notes |
|---|---|---|
| +0x00 | `short id` | from the global counter `pnl_id` (@0x10000444, starts at 0); also the index into `pnl_table` **and** `acttable` |
| +0x04 | owning panel | set by `pnl_addact`; non-zero means the actuator was added to a panel. `reset()` only restores actuators with this set |
| +0x08 | parent actuator (for sub-actuators; ep also sets it on the slider's rate hslider) | |
| +0x18 | type code | 0x136 button, 0x137 toggle, 0x138 radio, 0x139 wide button, 0x5a vertical multislider, 0x60 multislider bar, 0x61 open bar, 1 hslider, 2 filled vslider, 0x50 dial, 0x46 palette, 0x8c frame, 0x33 floating puck, 1000 mouse, 0x15 label, 0x14 typein |
| +0x1c | "active" (currently grabbed) | |
| +0x20/+0x24 | x, y (panel units) | |
| +0x28/+0x2c | w, h | |
| +0x44 | **val** | |
| +0x48 | extval: the position of a multislider sub-bar | |
| +0x4c | initval, copied from val by `pnl_addact`/`pnl_addsubact` | used by reset |
| +0x50 / +0x54 | **maxval / minval** | default 1.0 / 0.0 |
| +0x5c | label (char*) | |
| +0x60 | `short key`, an IrisGL key device code (key equivalent) | |
| +0x64 | label placement (see §2.4) | |
| +0x80 | newvalfunc (mouse → value) | |
| +0x8c | drawfunc | |
| +0x90 / +0x94 / +0x98 | **downfunc / activefunc / upfunc** | dispatched by `pnl_newvalact` @ 0x419584: down on the press frame, active on every frame while held, up on release |
| +0xa0 | `u`, a user pointer (ep stores an index, a string or a struct here) | |
| +0xa4 | type-specific data | |
| +0xac | "auto" actuator (its activefunc runs every frame from `pnl_doautos`) | |
| +0xb4 | visible | `togglechanmode` flips this |
| +0xbc | radio-group ring link | |

A toggle reads as "on" when `val > (max+min)/2`. ep checks every button this way, e.g.
`stopbutton` in `main`. libpanel draws a toggle or button as **unlit when `val == minval`**
and lit otherwise (`_drawtogglebutton`, `_drawbutton`).

### 2.2 Value behaviour per actuator type

* **button** (`pnl_button` @ 0x422144, `_newvalbutton` @ 0x41fd38): momentary. `val = max`
  while the mouse is down and `min` once it is released.
* **toggle** (`pnl_toggle_button` @ 0x4221c4, `_newvaltogglebutton` @ 0x41fda0): on the press
  frame, `val` flips between min and max (`val==min ? max : min`). 0.4 × 0.4 units.
* **wide button** (`pnl_wide_button` @ 0x422200): a momentary button 1.8 × 0.4 with the label
  centred inside. ep turns `auto` into a toggle by replacing newvalfunc with
  `_newvaltogglebutton`.
* **radio** (`pnl_radio_button` @ 0x422180, `_newvalradiobutton` @ 0x41fe1c): pressing one sets
  it to max and the rest of its ring to min. The ring is every radio button added to the panel
  since the last `pnl_endgroup` (ep never calls `pnl_endgroup`).
* **dial** (`pnl_dial` @ 0x427f10, `_newvaldial` @ 0x427668): relative rotary drag. The value
  changes by `Δangle / (2π·0.88) × (max−min)`; Ctrl scales this by 0.05. The value is clamped
  to [min,max]. Size 0.8 × 0.8, label below.
* **hslider / filled vslider** (`setsliderval` @ 0x428a84): an absolute jump on the press (unless
  data flag bit 2 is set, as on ep's rate sliders, which drag relatively), then relative drag,
  clamped. Ctrl gives 0.05× fine control.
* **palette** (`setpaletteval` @ 0x426504): a slider whose track is drawn as a colour-index ramp
  from min to max.
* **floating puck** (`_newvalfloatingpuck` @ 0x424e48): while dragged,
  `data.x = (pointer_x − puck.x − 0.4)/10 × (max−min)`, and likewise for y. The values are not
  clamped. ep reads `data+8` (x) and `data+0xc` (y).
* **mouse** (`pnl_mouse` @ 0x424480): an invisible pseudo-actuator. It becomes active whenever
  the left mouse button goes down in a window that is **not** a panel (here, the ep graphics
  window). `val` is 1 while held.

### 2.3 Key equivalents

`pnl_addact` → `pnl_add_key_equivalent` @ 0x419204 calls `qdevice(key)` and puts the actuator
on `pnl_kl`. `pnl_getactionfromqueue` @ 0x41c8b8 treats that key's press as `justdown` and its
release as `justup` on the actuator. Panel visibility is not checked, so the keys work while
the panels are hidden or in full-screen mode. A key works only if its actuator was **added**,
which is why "fade (W)" has no key in RGB mode. libpanel also queues LEFTMOUSE (tied to
MOUSEX/MOUSEY), both SHIFT keys, CTRLKEY and RIGHTCTRLKEY (`pnl_init` @ 0x41b914).

### 2.4 Label placement codes (+0x64, `pnl_labeloffsets` @ 0x4198bc)

0 = right of the actuator, centred vertically; 8 = left, centred vertically; 0xc = below,
centred; 0xd = below, right-aligned; 0x10 = centred inside; 0x11 = at the origin; 4 = above,
centred. Defaults by type: button/toggle/radio 0, wide button 0x10, slider/dial/palette/
multislider 0xc, frame 4, label 0x11. In the main panel, ep sets the toggles to 8, so their
labels sit to the left.

---

## 3. The composite "ep slider" (`mkepslider` @ 0x414488)

### 3.1 Structure

Each call creates **seven** actuators with consecutive ids. These offsets matter for scripts:

| id offset | role | type | geometry (relative to slider origin x,y) | behaviour |
|---|---|---|---|---|
| +0 | **M**, the main slider; the returned global (e.g. `zoomslider`) | vertical multislider (0x5a), labeltype 0xc (label below) | x, y, w 0.8, h 5.8 | holds min (+0x54), max (+0x50), val/initval = initial; `M->u` (+0xa0) points to the 0x24-byte struct below |
| +1 | upper bound ("hi") | open bar, sub-actuator of M | | extval starts at max; downfunc `saverange` |
| +2 | lower bound ("lo") | open bar, sub-actuator of M | | extval starts at min; downfunc `saverange`, activefunc `setcenter` |
| +3 | **value bar** | bar, sub-actuator of M | | extval = initial value. **This is the value the engine reads.** |
| +4 | **rate** | hslider (data flag 4: relative drag) | x, y+5.9, w 0.8, h 0.29 | min 0, **max (max−min)/2**, val 0; downfunc `saverange`, activefunc `setrange`; parent pointer = M |
| +5 | **bounce/wrap** toggle | toggle | x, y+6.29, w 0.8, h 0.29 | **minval 1.0, maxval 0.0 (inverted)**, val = bounce argument |
| +6 | **"R"** reset | wide button via `ep_mkuseract(0xd4, …)` | x, y+6.68, w 0.8, h 0.4 | downfunc `resetslider`; extra fields +0xc4 = M, +0xc8 = u-pointer (`float*` or 0), +0xcc = reset value (= initial) |

The struct at `M->u` (`pnl_alloc(0x24)`): +0x04 saved centre, +0x08 saved width (hi−lo),
+0x0c direction ±1.0 (starts at +1), +0x10 → hi bar, +0x14 → **value bar**, +0x18 → lo bar,
+0x1c → rate hslider, +0x20 → bounce toggle. The engine therefore reads a slider as
`*(*(M+0xa0)+0x14)+0x48`, the value bar's extval. `M.val` mirrors it once
`animateslider` has run.

M's multislider data has flags = 1 ("push" mode), bar count 0, bar thickness 0.145. Every
slider is also pushed onto `modsliderlist`, and every R button onto `rblist`.

### 3.2 Interaction semantics

* **Dragging on M** grabs the nearest of the three bars (`_newvalvmultislider` @ 0x4227a0).
  Push mode (`setsubactextval` @ 0x4224cc) keeps the order lo ≤ value ≤ hi: a dragged bar
  shoves its neighbours along and is clamped to [min,max]. M.val follows the grabbed bar.
* **Dragging the lower bound** moves the whole window. `setcenter` @ 0x40b69c sets
  hi = lo + the width saved at mouse-down, clamps hi to max, then clamps the value into [lo,hi].
  **Dragging the upper bound** changes only the width (its only callback is `saverange` on
  down).
* **Rate slider** (+4): `setrange` @ 0x40b894 only re-clamps. The rate value drives
  auto-oscillation in `animateslider` @ 0x40bc8c, which `main` calls for every slider in
  `modsliderlist` each frame:
  `value += dir × rate × (hi−lo)/(max−min) × speed`, where `speed` is the value bar of
  `speedslider`. Nothing moves while rate = 0 or while the value bar itself is being dragged.
  * bounce toggle **val > 0.5 (= 1)**: the value reflects off lo/hi and dir flips (ping-pong);
  * **val = 0**: the value wraps from hi back round to lo, and vice versa.
  * Because minval = 1, a bouncing toggle draws **unlit** and a wrapping toggle draws **lit**.
* **R** (`resetslider` @ 0x40ad9c) works in two stages. The first press sets hi = max,
  lo = min, rate = its initval, and value bar = M's initval if it is not already there. A press
  when the value already equals the initval writes the reset value through the u-pointer
  instead (e.g. `wheel = 0`), which resets the accumulated angle of whl/spn/flp.
  **UNCERTAIN/bug:** for the four texture sliders the u-pointer is the *address of a
  ring-buffer pointer* (`texturet`, `textures`, `texturesmax`, `texturetmax`, allocated by
  `pnl_alloc(0x200)` in `main`). The second stage would overwrite those pointers with 0.0 or
  1.0f bits, and the global `reset` (which presses every R twice) would do the same. On real
  hardware this would probably fault on the next frame. The port should reset the per-strand
  texture offsets instead. Running "reset" on IRIX would confirm the crash.

### 3.3 Arguments of every `mkepslider` call (from the machine code)

Signature: `mkepslider(label, x, y, min, max, initial, float *u, int bounce, panel)`.

| slider (global) | label | x | y | min | max | initial | u-pointer | bounce |
|---|---|---|---|---|---|---|---|---|
| `zoomslider` | `zoom` | 0 | 3.5 | −0.5 | 0.5 | 0 | – | 1 |
| `twistslider` | `twst` | 1 | 3.5 | −200 | 200 | 0 | – | 1 |
| `speedslider` | `spd` | 2 | 3.5 | 0 | 1 | 1 | – | 1 |
| `nslider` | `n` | 3 | 3.5 | 1 | 127 | 32 | – | 1 |
| `ambientslider` (lit build) | `amb` | 4 | 3.5 | 0 | 1 | 0 | – | 1 |
| `shinyslider` (lit build) | `shn` | 5 | 3.5 | 0 | 1 | 1 | – | 1 |
| `maprateslider` (unlit build only, dead) | `rat` | 4 | 3.5 | −20 | 20 | 1 | – | 1 |
| `maprangeslider` (unlit build only, dead) | `rng` | 5 | 3.5 | 0 | 100 (0–10 if `getplanes()==4`) | 1 | – | 1 |
| `wheelslider` | `whl` | 0 | 11.5 | −60 | 60 | 0 | `&wheel` | 1 |
| `spinslider` | `spn` | 1 | 11.5 | −20 | 20 | 0 | `&gspin` | 1 |
| `flipslider` | `flp` | 2 | 11.5 | −20 | 20 | 0 | `&gflip` | 1 |
| `armslider` | `arm` | 3 | 11.5 | −3 | 3 | 0 | – | 1 |
| `wristslider` | `wrst` | 4 | 11.5 | −3 | 3 | 0 | – | 1 |
| `sizeslider` | `sz` | 5 | 11.5 | 0 | 10 | 1 | – | 1 |
| `hslider[i]`, i=0..4 | (none) | i | 3.0 | 0 | 2 | 1 | – | **0** (wrap) |
| `lslider[i]`, i=0..4 | (none) | i | 3.0 | 0 | 1 | 0.5 | – | **0** (wrap) |
| `texturetslider` | `t` | 0 | Y+0.5 | −0.01 | 0.01 | 0 | `&texturet` | 1 |
| `texturesslider` | `s` | 1 | Y+0.5 | −0.01 | 0.01 | 0 | `&textures` | 1 |
| `texturesmaxslider` | `S` | 2 | Y+0.5 | 0 | 10 | 1 | `&texturesmax` | 1 |
| `texturetmaxslider` | `T` | 3 | Y+0.5 | 0 | 10 | 1 | `&texturetmax` | 1 |

Notes:
* `spn`, `flp` and `rat` all take −20 as their min. The value is set in the delay slot of the
  `has_lighting` branch at 0x415444; a naive linear trace reads 11.5 instead.
* After the colour loop, `defpanel` sets **`hslider[0]`'s rate slider to 0.01** (val and
  initval). Hue channel 0 therefore drifts on its own unless a script zeroes it, as the default
  script does with `actset: 127, 0.0`.
* `lslider[i]`, its rate slider and its bounce toggle start **hidden** (`+0xb4 = 0`). The R
  buttons stay visible. See `togglechanmode`, §4.2.
* In the texture panel, Y is the running y after the radio buttons (§4.3).

---

## 4. Panels

Panels are created in this order: scripting, electro-paint, colors, texture. `pnl_mkpanel`
prepends to `pnl_pl`, so `pnl_init` gives windows to the newest panel first. Each panel is
its own IrisGL window (`pnl_initpanel` @ 0x41a470): colour-index, double-buffered, title =
panel label, size = 40 px × (actuator bounding box + 0.2 units of margin), `keepaspect` to that
size. The first panel window is placed 12 px to the right of the ep window and top-aligned
with it. Later panels stack beneath it and wrap to a new column when they run out of room.
Windows are created only for panels whose visible flag (+0x70) is set.

Visibility: `defpanel` clears `+0x70` on every panel when `cflag == 0`, so all panels start
**hidden unless `-c`**. `fullscreen()` hides every panel and `regularscreen()` shows every
panel, *including the scripting panel, even without `-c`* (§6).

Panel callbacks (`pnl_dofuncs`; panel +0x64/+0x68/+0x6c = down/active/up):
* electro-paint: active = up = `recordact` @ 0x40d280
* colors: active = `coloractivefuncs` @ 0x40e740 (`constraincolorpanel` → `chooseupdates` on
  the current actuator, then `recordact`); up = `recordact`
* scripting, texture: none

### 4.1 Panel "electro-paint" (`panel`, id 12)

| id | global | label (exact) | type | x, y | min–max | initial | key | callbacks / notes |
|---|---|---|---|---|---|---|---|---|
| 13 | – | `mouse` | mouse | – | 0–1 | 0 | – | down: `setfullscreen`. A click in the ep window toggles full-screen (not in `-S`) |
| 14 | – | `...by David Tristram` | label | 0.2, 19.0 | – | – | – | static text |
| 15 | – | `exit` | wide button | 0, 0 | 0–1 | 0 | ESC (7) | up: `cleanexit` |
| 16 | – | `reset` | wide button | 0, −1.0 | 0–1 | 0 | – | up: `reset` @ 0x40aecc |
| 17 | `autobutton` | `auto` | wide button made a toggle | 0, −1.5 | 0–1 | 0 (off) | – | up: `setauto` @ 0x40d654 (turning it off → `stopAnim`) |
| 18 | `outlinebutton` | `outline (O)` | toggle, label left | 4.5, 2.5 | 0–1 | 0 | O (41) | per-frame → `outline[n]` |
| 19 | `fatlinebutton` | `fat lines (I)` | toggle | 4.5, 2.0 | 0–1 | 0 | I (40) | → `fatline[n]` |
| 20 | `fillbutton` | `fill (U)` | toggle | 4.5, 1.5 | 0–1 | **1 (on)** | U (33) | → `fill[n]` |
| 21 | `ribbonbutton` | `ribbons (R)` | toggle | 4.5, 1.0 | 0–1 | 0 | R (24) | added only if `has_tmesh`; read by `drawit` |
| 22 | `smoothbutton` | `smooth (M)` | toggle | 4.5, 0.5 | 0–1 | 0 | M (44) | down: `setsmooth` @ 0x40a1a8 |
| 23 | `smearbutton` | `smear (Q)` | toggle | 4.5, 0.0 | 0–1 | 0 | Q (10) | down: `setsmear` @ 0x40a2b8 |
| 24 | `fadebutton` | `fade (W)` (label only when `rgb_mode==0`) | toggle | 4.5, −0.5 | 0–1 | 0 | W (16) | **created but not added in this build**; −0.5 is its CI-build slot |
| 25 | `backgroundbutton` | `back (E)` | toggle | 4.5, −0.5 | 0–1 | 0 | E (17) | background colour cycling |
| 26 | `zbufbutton` | `z-buf (Z)` | toggle | 4.5, −1.0 | 0–1 | 0 | Z (20) | added only if `has_zbuffer`; down: `setzbuf` @ 0x40a494 |
| 27 | `lightbutton` | `light (L)` | toggle | 4.5, −1.5 | 0–1 | 0 | L (42) | added only if `has_lighting`; read by `drawit`/`drawribbons` |
| 28 | `stopbutton` | `stop (S)` | toggle | 4.5, −2.0 | 0–1 | 0 | S (12) | pauses animation (§7) |
| 29 | `fullbutton` | `full screen (F)` | toggle | 4.5, −2.5 | 0–1 | 0 | F (19) | down: `setfullscreen` @ 0x40abc8 |
| 30 | `positionpuck` | (none) | floating puck 0.8×0.8 | 0, 1.5 | 0–1 (scale) | (0,0) | – | per-frame → `x[n]`, `y[n]` |
| 31–37 | `zoomslider` | `zoom` | ep slider | 0, 3.5 | −0.5–0.5 | 0 | – | → `dzoom[n]` |
| 38–44 | `twistslider` | `twst` | ep slider | 1, 3.5 | −200–200 | 0 | – | → `dtwist[n]` |
| 45–51 | `speedslider` | `spd` | ep slider | 2, 3.5 | 0–1 | 1 | – | global time scale (§7) |
| 52–58 | `nslider` | `n` | ep slider | 3, 3.5 | 1–127 | 32 | – | `nlimit = floor(value)` |
| 59–65 | `ambientslider` | `amb` | ep slider | 4, 3.5 | 0–1 | 0 | – | → `ambient[n]` |
| 66–72 | `shinyslider` | `shn` | ep slider | 5, 3.5 | 0–1 | 1 | – | → `shiny[n]` |
| 73–79 | `wheelslider` | `whl` | ep slider | 0, 11.5 | −60–60 | 0 | – | `wheel -= speed·whl` per frame |
| 80–86 | `spinslider` | `spn` | ep slider | 1, 11.5 | −20–20 | 0 | – | `gspin += spn` per new segment → `spin[n]` |
| 87–93 | `flipslider` | `flp` | ep slider | 2, 11.5 | −20–20 | 0 | – | `gflip += flp` per new segment → `flip[n]` |
| 94–100 | `armslider` | `arm` | ep slider | 3, 11.5 | −3–3 | 0 | – | → `arm[n]` |
| 101–107 | `wristslider` | `wrst` | ep slider | 4, 11.5 | −3–3 | 0 | – | → `wrist[n]` |
| 108–114 | `sizeslider` | `sz` | ep slider | 5, 11.5 | 0–10 | 1 | – | → `size[n]` |

Without a z-buffer (Indigo/CI) the toggles below z-buf move up 0.5: light −1.0, stop −1.5,
full −2.0. The ids stay the same.

Button callbacks:
* `setsmear` (down): smear on and smooth off → clear the screen (unless stopped), draw once, and
  draw into both buffers (`frontbuffer(1); backbuffer(1)`). Smear off → back buffer only.
* `setsmooth` (down, effective only while smear is on): smooth on → back buffer only; off →
  both buffers.
* `setzbuf` (down): `zbuffer(on)`, plus `zclear()` when switching on.
* `setauto` (up): turning auto off calls `stopAnim` @ 0x40d5ec, which clears the "animating"
  flag on every `acttable` record.
* `reset` (up): presses every R button in `rblist` **twice** (bounds, rate and value back to
  initial, then u-pointer reset values). It also zeroes `dwheel`, recentres the puck, and
  restores initval on smear/zbuf/smooth/outline/fill/fatline/fade/ribbon/back/light, the
  dials, `adial[]`, `dwellslider[]`, chanrate and the two palettes, re-running each callback.
  It ends with `togglechanmode(lightpalette)`, which returns to hue mode. It never touches
  `stopbutton`, `fullbutton`, `autobutton`, `activebutton[]` or anything in the texture panel.
  It also checks the never-built globals `alphadial` and `hueratedial`.

### 4.2 Panel "colors" (`panel`, id 115; RGB builds only)

| id | global | label | type | x, y | min–max | initial | callbacks / notes |
|---|---|---|---|---|---|---|---|
| 116 | `alphaoutdial` | `outa` | dial | 1, 0 | 0–1 | 1.0 | → `alphaout[n]` |
| 117 | – | `R` | wide button (useract, w 0.8) | 1, 1.0 | – | – | down: `resetact` @ 0x40ad14 → dial back to its initval |
| 118 | `alphabackdial` | `baka` | dial | 2, 0 | 0–1 | 1.0 | background alpha (`bkgRGBA[3]`) |
| 119 | – | `R` | wide button | 2, 1.0 | – | – | `resetact` |
| 120 | `huebackratedial` | `outr` | dial | 3, 0 | −20–20 | 0.2 | `bhue += speed × outr × 0.05` per frame |
| 121 | – | `R` | wide button | 3, 1.0 | – | – | `resetact` (reset value 0 but no u-pointer, so no second stage) |
| 122 + 18i (i=0..3), 194 (i=4) | `adial[i]` | (none) | dial | i, 2.0 | 0–1 | 1.0 | alpha of channel i → `alpha[n]` when i is the current channel |
| 123+18i / 195 | `hslider[i]` (7 ids) | (none) | ep slider | i, 3.0 | 0–2 | 1 | hue of channel i (`fmod(v,1)`); wrap mode |
| 130+18i / 202 | `lslider[i]` (7 ids) | (none) | ep slider, **hidden** | i, 3.0 | 0–1 | 0.5 | lightness of channel i; wrap mode |
| 209 | `huepalette` | (none; u = `"h"`) | **frame** with drawfunc `drawdoublepalette` @ 0x409c44 | 5.0, 3.0, w 0.3, h 5.8 | basecolor … basecolor+ncolors−1 | – | down: `togglechanmode` → switch to **lightness** mode |
| 210 | `lightpalette` | (none; u = `"l"`) | palette, **hidden** | 5.0, 3.0, w 0.3 | 512 … 638 (colour indices) | – | down: `togglechanmode` → switch to **hue** mode |
| 137+18i / 211 | `hueindex[i]` | (none) | palette 0.8×0.4 (a swatch) | i, 10.28 | 768+i … 768+i | – | shows colour index 0x300+i, which `main` sets each frame to hls(hue_i, light_i) |
| 138+18i / 212 | `dwellslider[i]` | (none) | filled vertical slider 0.8×2.8 | i, 10.88 | 0–1 | 1 for i=0, else 0 | relative dwell time of channel i; `updatedwellsliders` @ 0x4094a0 renormalises the others |
| 139+18i / 213 | `activebutton[i]` | (none) | button (momentary) 0.8×0.4 | i, 13.88 | 0–1 | 1 for i=0, else 0 | holding it forces the current channel to i; otherwise lights the current channel |
| 214 | `chanrateslider` | (none) | hslider, w 4.8, h 0.29 | 0, 14.48 | 0–1 | 0 | `gchant += chanrate × speed` per frame (channel sequencer) |

Channel ids: i=0 → adial 122, hslider 123–129, lslider 130–136, hueindex 137, dwell 138,
active 139. i=1 → 140, 141–147, 148–154, 155, 156, 157. i=2 → 158, 159–165, 166–172, 173, 174,
175. i=3 → 176, 177–183, 184–190, 191, 192, 193. i=4 → 194, 195–201, 202–208, then the
huepalette 209 and lightpalette 210, then 211, 212, 213.

Behaviour:
* **Hue/lightness mode switch** (`togglechanmode` @ 0x409764, keyed on the first character of
  `u`). `'h'` hides the hue frame, shows the light palette, hides all `hslider[i]` (M, rate,
  bounce) and shows all `lslider[i]`. Any other value does the reverse. The two sets of
  sliders overlap at the same position, so the port can model one set of five columns with a
  hue/lightness mode switch.
* **Dwell normalisation** (`updatedwellsliders`, called by `chooseupdates` when a dwell slider
  is the current actuator, and by `setact` for type-2 sliders). When slider k moves by Δ from
  its remembered value, every other slider j becomes `v_j − Δ·v_j/Σ_{others}`, or `v_j − 0.2Δ`
  if the others sum to 0, clamped to [0,1]. All five values are then remembered (array @
  0x10004b88). **Quirk:** the remembered values start at 0, so the first adjustment treats the
  whole value of slider k as Δ.
* **Active buttons** (`updateactivebutton` @ 0x4096d8): pressing one resets the other four to
  min.
* **Channel selection** (in `main`): if the current actuator is `activebutton[i]`, then
  channel = i. Otherwise the channel is the first i whose cumulative dwell exceeds `gchant`,
  falling back to 0. If that channel's button is not lit, all buttons are cleared and it is
  lit. Per frame: `ghue = hslider[ch]`, `glight = lslider[ch]`, `alpha[n] = adial[ch]`.

### 4.3 Panel "texture" (`panel`, id 315)

`defpanel` first sets `pnl_id = 115 + 200 = 315` (see §5).

| id | global | label | type | x, y | min–max | initial | key | notes |
|---|---|---|---|---|---|---|---|---|
| 316 | `texturebutton` | `no texture` | radio | 0, Y0 | 0–1 | **1 (selected)** | – | u = 0; down: `settextureindex` @ 0x40bc64 → `currenttextureindex = u` |
| 317 | `trianglebutton` | `triangles <T>` | toggle | 0, Y0+0.6 | 0–1 | 0 | T (25) | read by `drawshape` |
| 318 … 317+N | – | the file name (e.g. `foo.rgb`) | radio | 0, Y0+1.2+0.6k | 0–1 | 0 | – | `-t` only; u = texture index 2, 3, … |
| 318+N … +27 | `texturetslider`, `texturesslider`, `texturesmaxslider`, `texturetmaxslider` | `t`, `s`, `S`, `T` | ep sliders | x 0,1,2,3; y = last radio y + 0.6 + 0.5 | see §3.3 | | – | → `dtexturet[n]`, `dtextures[n]`, `texturesmax[n]`, `texturetmax[n]` |

Y0 is the running y left after the colour panel: 14.48 + 0.29 = 14.77. All radio buttons
(the "no texture" button plus the images) share one ring.

**`-t` file scan** (in `defpanel`): `opendir(".")`, then `readdir` in directory order (unsorted,
no `closedir`). Every entry whose name ends in `.rgb` (`endsin` @ 0x4148e0, case-sensitive
`strcmp` of the tail) gets the next index (starting at **2**, so index 1 is never used) and is
loaded by `simpletextureread` @ 0x417994. That function reads the SGI image; if either
dimension exceeds 256 it runs `system("izoom <file> /tmp/z.rgb <256/xsize> …")` and loads
`/tmp/z.rgb` as 256×256. It calls `texdef2d(index, 4 components, w, h, …)` with one property
set for exactly 256×256 and another for other sizes, sets up the texture environment once, and
then binds. A radio button labelled with the entry name follows. After the scan,
`simplesettexture(0)` unbinds.
**UNCERTAIN:** the label pointer is the name inside the `dirent` buffer, which a later
`readdir` may overwrite, so long directories could garble labels. The second `izoom` scale
argument is not visible in the decompile.

### 4.4 Panel "scripting" (`scriptpanel`, id 0; `initscriptpanel` @ 0x4271ec)

These are libpanel's own controls for binary scripts. They are made with `pnl_mkact`, **not**
`ep_mkact`, so they are not in `acttable` (ids 1–11).

| id | global | label | type | x, y | callback |
|---|---|---|---|---|---|
| 1 | `filetypein` | `script file` | typein, label below right (0xd), default text `panel.script` | 0, −0.5 | – |
| 2 | – | `begin write` | button | 0, −1.0 | down: `beginscript` → `pnl_beginwritescript(text)` |
| 3 | – | `begin append` | button | 0, −1.5 | down: `appendscript` |
| 4 | – | `stop write/append` | button | 0, −2.0 | down: `endscript` |
| 5 | – | `begin read` | button | 0, −3.0 | down: `readscript` → `pnl_beginreadscript(text)` |
| 6 | – | `continue read` | button | 0, −3.5 | down: `continuescript` |
| 7 | – | `stop read` | button | 0, −4.0 | down: `stopscript` |
| 8 | – | `READING` | wide button used as a lamp (no newvalfunc, auto) | 4, −1.5 | active: `setreadbutton` (lit while a script is being read) |
| 9 | – | `WRITING` | wide button lamp | 4, −2.0 | active: `setwritebutton` |
| 10 | – | `DONT DRAW` | wide toggle, w 3.0 | 4, −3.5 | up: `setdontdraw` → `pnl_dont_draw` |
| 11 | – | `IGNORE DELAYS` | wide toggle, w 3.0 | 4, −4.0 | up: `setignoredelay` → `pnl_ignore_delay` |

Binary script format (`pnl_putactiontoscript` @ 0x41ba48 / `pnl_getactionfromscript` @
0x41d204): a delay record is magic 0xabc2 followed by an int32 frame count. An action record
is magic 0xabc1 followed by 9 words: panel id (or −1), actuator id, x, y (float),
justup, justdown, mousedown, shift, ctrl. 0xabc0 marks a state dump. All values are
big-endian. Ids are `pnl_table` indices, which use the same numbering as `acttable`.

### 4.5 Declared but never constructed in the 5.3 binary

`alphaslider`, `alphaoutslider`, `backrateslider`, `alphadial`, `hueratedial`,
`spectrumbutton`, `contourbutton`, `bandbutton`, `readtexturebutton`, `maprateslider` and
`maprangeslider` (both colour-index only). `load_contour_map` @ 0x409068 and `load_band_map` @
0x40925c have no callers. The port should not show any of these.

---

## 5. Actuator table (`acttable` @ 0x10003988, 0x400 slots)

`ep_mkact` @ 0x41432c calls `pnl_mkact(type)`, allocates a 0x28-byte record and stores it at
`acttable[act->id]`, then updates `acttablemaxindex`. Ids come from libpanel's single counter
`pnl_id`, which **panels also consume**, as do the scripting-panel actuators built with
`pnl_mkact`. `defpanel` pins two bases:

* `iVar1 = pnl_id` after the main panel is 115. The colors panel uses ids 115–214.
* `pnl_id = 115 + 200 = 315` before the texture panel, and `pnl_id = 115 + 400 = 515` at the
  end. Any later allocation, such as a multislider click with flag bit 4 (which ep never sets),
  starts at 515.

Because the main panel always builds the same actuators (some are created but not added), the
ids are fixed for every build. The only variable range is the texture panel after id 317,
which shifts by the number N of `.rgb` files.

**Validation:** the built-in script (`defaultScript` @ 0x10000410 → text @ 0x100010d0; identical
in the 6.5 build) uses these ids with comments: 29 "full", 17 "auto", 18 "outline", 73 "whl",
31 "zoom", 38 "twst", 80 "spn", 108 "size", 52 "n", 101 "wrst", 127 "hue rate chan 0", 123
"hue chan 0", 130 "lightness chan 0", 87 "flip", 94 "arm". All of them agree with the
derivation above.

Record layout (int/float words): [0] actuator, [1] animating flag, [2] in-flight flag, [3]
direction ±1, [4] current target, [5] limit 2 (high), [6] limit 1 (low), [7] per-frame step,
[8] duration in frames, [9] variance. The record is calloc'd and `ep_mkact` sets only [0] and [1], so these start at 0 until an `actlim2` (or a recorded release) copies them in; 300 is the default of a *sequence* and the fallback of `recordact`.

**`ep_mkuseract` quirk** (@ 0x4143b8): it copies the freshly made actuator into a larger
0xd4-byte block. The panel gets the copy, but `acttable[id]` and `pnl_table[id]` keep the
**original**, which is never added and has no label. Script commands aimed at an "R" button
id (+6 of every ep slider, and 117/119/121) therefore act on an orphan and have no visible
effect.

### 5.1 `-o` table dump (printed in `main` after `defpanel`)

Format: `"#\n# actuator table begin\n#\n"`, then for every non-NULL slot from 0 to 0x3ff
`"# %d: %s\n"` (index, label of `record[0]`), then `"#\n# actuator table end\n#\n"`.
Expected lines for the default build (z-buffer present, no `.rgb` files):

```
13 mouse, 14 ...by David Tristram, 15 exit, 16 reset, 17 auto, 18 outline (O),
19 fat lines (I), 20 fill (U), 21 ribbons (R), 22 smooth (M), 23 smear (Q), 24 <NULL>,
25 back (E), 26 z-buf (Z), 27 light (L), 28 stop (S), 29 full screen (F), 30 <NULL>,
31 zoom, 38 twst, 45 spd, 52 n, 59 amb, 66 shn, 73 whl, 80 spn, 87 flp, 94 arm, 101 wrst, 108 sz,
116 outa, 118 baka, 120 outr, 316 no texture, 317 triangles <T>, 318 t, 325 s, 332 S, 339 T
every other index in 31–114, 116–214 and 318–345 → <NULL>   (indices 0–12, 115, 215–315 absent)
```

**UNCERTAIN:** about 290 slots have a NULL label, and the format string has no guard. The
output depends on IRIX 5.3 libc's `printf("%s", NULL)`, which may print `(null)` or may fault.
Running `ep -o` on IRIX would resolve it (`recordact` does guard with `"<no label>"`).

### 5.2 ep script commands that use acttable indices (`readAnimation` @ 0x40c0d4, `processCommand` @ 0x40da04)

* `actset: idx, v` → `setact` @ 0x40d824. For an ep slider (type 0x5a) it sets the value bar,
  clamped to [lo,hi], and copies it to M.val. For other types it sets val; type 2 (dwell) also
  runs `chooseupdates`. It then calls the actuator's down, active and up callbacks, so
  `actset: 29, 1.0` enters full-screen mode.
* `actlim1: idx, v` sets limit 1, clamped to ≥ min. `actlim2: idx, v` sets the sorted limit
  pair, clamped to ≤ max, copies duration and variance from the sequence, and enables
  animation of that record.
* `actreset: idx` restores val to initval. `actresetall:` calls `reset()`. `actstop: idx`
  clears the animating flag. `actstopall:` calls `stopAnim()`.
* The other commands (`frame:`, `relframe:`, `absframe:`, `randdelay:`, `duration:`,
  `variance:`, `seqname:`, `seqdo:`, `seqloop:`, `seqstop:`, `seqkill:`, `seqkillall:`, `#`
  comments) do not take indices. `seqrepeat:` is parsed but reported as "not implemented".
* While animating (`setacttargets` @ 0x40e3d8, `animateacts` @ 0x40e630), each enabled record
  picks a uniform random target between its limits and steps linearly toward it over about
  `duration × (exponential random, weighted by variance)` frames, at least 1. On arrival it
  picks a new target. **Off-by-one:** both loops run `i < acttablemaxindex`, so the highest id
  is never animated.

Teach-by-demonstration with `-o` (`recordact`, the main panel's active and up callback):
pressing an actuator prints `actlim1: id, val\t# "label"`, and with auto on also stores limit
1. Releasing prints `actlim2: …`, and with auto on **and `-o`** stores the sorted limits
(duration 300, or the current sequence's) and enables animation. A `relframe: n` line is
printed whenever more than one frame has passed since the last recorded action. Captured
stdout is therefore an ep script.

---

## 6. Command line (`main` @ 0x40eaec, `getopt "Scrabsqot"`)

| opt | effect on flags | behaviour |
|---|---|---|
| (none) | `aflag=1, bflag=1` from `.data` | the built-in "mello" script is parsed before any window opens and runs from frame 0 (`actset: 29, 1.0` → full-screen, auto 0, outline 1, zoom 0.06, twst 60, sz 1.2, n 40, sequences 1–8). Panels start hidden. |
| `-S` | `is_saver = 1` | screensaver mode, see below |
| `-c` | `aflag=0, bflag=0, cflag++` | **no script**. All four panels are visible at startup and the window is regular-sized, so this mode shows the "controls". |
| `-r` | `rflag++` | on the first loop iteration `main` calls `fullscreen()` and then `pnl_beginreadscript("panel.script")` (libpanel binary script, `pnl_scriptinfile` @ 0x1000049c). Does not clear aflag, so the internal script also runs unless `-c` comes after it. |
| `-a` | `aflag++, bflag=0, cflag=0` | reads an ep script from **stdin**, one command per line until EOF, before `winopen` |
| `-b` | `bflag++, aflag++, cflag=0` | built-in script (this is the default) |
| `-o` | `oflag++` | echoes script parsing (`# reading animation...`, every line, `# done reading animation`, `seq %d at %.2f` / `  cmd …` summaries), prints the actuator table, traces each processed command (`proc: currentFrame …`), and enables recording output (§5.2) |
| `-s` | `qflag=0, sflag++` | single strand (default, `sflag=1`) |
| `-q` | `sflag=0, qflag++` | quad strand. Only `sflag` is read later (`drawit`, `drawribbons`), so `qflag` is otherwise unused. |
| `-t` | `tflag++` | adds the `./*.rgb` texture buttons (§4.3) |
| other | – | prints the usage text to stderr and exits with status 1. The text has a typo: the quad-strand line says `-s`. Exact lines: `ep: usage: ep [-Scrabsqo]`, `\t-r: start reading "panel.script", panel library script file`, `\t-a: read ep script on stdin`, `\t-b: read internal ep script <default>`, `\t-c: start up with controls visible`, `\t-o: write script-like junk to stdout`, `\t-s: single strand mode <default>`, `\t-s: quad strand mode`, `\t-S: screen saver mode (for use with haven)`, `\t-t: enable textures, (reads ./*.rgb)` |

Options are processed left to right, so the last of `-a`/`-b`/`-c` decides the script and
panel state.

### 6.1 Screensaver mode `-S`

* Window: `foreground()` (no fork into the background), `noborder()`, `winopen("ep")` with
  **no** `prefposition`, `keepaspect` or `winattach`.
  **UNCERTAIN:** how the window gets its size and position (presumably the haven saver
  launcher or the window manager; plain IrisGL would ask the user to sweep it out). Running it
  under haven on IRIX would settle this.
* Events: only REDRAW (528/0x210) is registered as a user window event. WINFREEZE and WINTHAW
  are not, and `load_spectrum_map` is skipped.
* `setfullscreen` does nothing (`is_saver` check), so neither the "F" key, the mouse
  pseudo-actuator nor the script's `actset: 29, 1.0` changes the window. `fullbutton` still
  ends up at val 1, which makes REDRAW use the screen aspect ratio (§7.1).
* Panels stay hidden (`cflag = 0`). The key equivalents are still queued.
* Exit: ESC triggers the "exit" button's upfunc `cleanexit` on key release, which restores the
  colormap and exits with status 0. ep installs no signal handlers (no `signal`/`sigset`
  import). Otherwise the saver manager has to kill it, and the colormap is not restored.
  **UNCERTAIN:** whether haven kills ep or sends it ESC.

---

## 7. Window, screen and event handling

### 7.1 Normal window (`main`)

* `regx = regy = 5`. `regw = regh = getgdesc(GD_YPMAX) − 29`, a square window as tall as the
  screen minus 29 px (IrisGL origin is bottom-left). Then `prefposition(5, 5+regw−1, 5,
  5+regh−1)`, `winopen("ep")` (title **"ep"**), `keepaspect(1,1)`, `winconstraints()`,
  `winattach()`.
* GL state: `doublebuffer`, `mmode(MVIEWING)` + `deflight` (lit build), `RGBmode` unless
  CI-only, `blendfunction(BF_SA, BF_MSA)` unless Indigo/CI, `subpixel(1)`, `gconfig`.
  Initial projection: `perspective(300, 1.0, 0.01, 1e6)`; `polarview(10, 0, 0, 0)`;
  `lsetdepth(0, 0x7fffff)`. The front and back buffers are both cleared to the background.
* Colour indices (`getplanes`): 4 planes → `ncolors 15`, `basecolor 1`, plus a libpanel colour
  remap; 8 planes → 128 from 128; otherwise 128 from **640 (0x280)**.
* **`savecolors`** @ 0x40baa0 runs before `defpanel` in every mode. It saves `getmcolor` for
  `basecolor … basecolor+ncolors−1` into three short arrays @ 0x10004ba0/0x10006ba0/0x10008ba0.
  **`restorecolors`** @ 0x40bb64 writes them back. Only `cleanexit` @ 0x40bc1c calls it. ep
  also rewrites indices 768–772 (0x300+i, the channel swatches) every frame and never restores
  them. `load_spectrum_map` @ 0x408ef4 (not in `-S`) fills the base range with an HLS hue ramp
  (L=0.5), which the colors panel's hue frame displays.

### 7.2 `fullscreen` @ 0x40a574

1. Sets `fullbutton` to 1.
2. If `stopbutton` is on, it puts `fullbutton` back to 0 and returns. Full-screen is refused
   while stopped.
3. Hides every panel (visible = 0, `pnl_fixpanel`) and the cursor (`pnl_cursoff`, `cursoff`).
4. Saves the window's origin and size into `regx/regy/regw/regh`.
5. Calls `winconstraints()` to drop the aspect lock, then `winposition(0, XPMAX−1, 0,
   YPMAX−1)`, `reshapeviewport`, `perspective(300, XPMAX/YPMAX, 0.01, 10000)`, `loadident`,
   `polarview(10,0,0,0)`, `lsetdepth`, `winpop`.
6. Clears and swaps twice.

### 7.3 `regularscreen` @ 0x40a904

1. Sets `fullbutton` to 0.
2. Shows **every** panel (visible = 1), including the scripting panel and with or without
   `-c`, and turns the cursor back on.
3. Calls `winposition` with the saved region, then `reshapeviewport`, `perspective(300, 1.0,
   0.01, 10000)`, `loadident`, `polarview`, `lsetdepth`, `keepaspect(1,1)`, `winconstraints`.
4. Clears and swaps, sets main panel +0x78 = 2, runs `pnl_drawpanel`, clears and swaps again.
5. Calls `qenter(INPUTCHANGE=534, 1)`.

### 7.4 `setfullscreen` @ 0x40abc8

Does nothing in `-S`. Called from `fullbutton`, it follows the button's new state. Called from
the mouse pseudo-actuator or anything else, it toggles: a click on the graphics window in a
normal session swaps between full-screen and windowed (and so also hides or shows the panels).

### 7.5 Events (main loop)

Each iteration runs `pnl_dopanel` (`pnl_naptime = −1`, so it never sleeps), then
`pnl_userwinevent`:

| event (device.h) | when registered | handling |
|---|---|---|
| REDRAW 528 (0x210) | always | `reshapeviewport`. Perspective aspect is XPMAX/YPMAX if `fullbutton` is on, otherwise 1.0, far plane 10000. Then `loadident`, `polarview`, `lsetdepth`, clear to background. |
| WINFREEZE 539 (0x21b) | not in `-S` | sets a "frozen" flag. While it is set, each frame blocks in `qread` and puts the event back with `qenter`, so ep waits for the next event. |
| WINTHAW 540 (0x21c) | not in `-S` | clears the frozen flag |
| LEFTMOUSE 103 + MOUSEX/Y | libpanel | panel interaction. In a non-panel window it activates the `mouse` pseudo-actuator, so it calls `setfullscreen` |
| key equivalents (§4.1) | at `pnl_addact` | press = actuator down, release = up |
| SHIFT 5/6, CTRL 3, RIGHTCTRL 145 | libpanel | modifier state; Ctrl gives fine adjustment on sliders and dials |
| KEYBD 513 | only while a typein is active | script file name editing |

(The brief listed 0x21b/0x21c as INPUTCHANGE codes. They are WINFREEZE and WINTHAW;
INPUTCHANGE is 534/0x216, and ep only uses it in `regularscreen`'s `qenter`.)

### 7.6 Per-frame control → engine mapping (`main`)

| step | rule |
|---|---|
| stop | if `stopbutton` is on: clear unless smear is on, draw once, swap, then loop `pnl_dopanel` + `sginap(0)` + `swapbuffers` until it is off |
| auto | if `aflag` or `autobutton` is on: `tasteQueue`, `setacttargets`, `animateacts` |
| sliders | `animatesliders()` (rate/bounce on every ep slider) |
| time | `t += speed`. Each integer carry advances the segment counter `n`. Ring buffers are indexed `n & 127`. |
| channels | `gchant = fmod(gchant + chanrate·speed, 1)` |
| ring buffers | `x[n], y[n] ← puck`, `dzoom ← zoom`, `arm`, `wrist`, `dtwist ← twst`, `outline/fatline/fill ← buttons > 0.5`, `size ← sz`, `alpha ← adial[ch]`, `alphaout ← outa`, `shiny ← shn`, `ambient ← amb`, `hue ← ghue`, `light ← glight`, `textureindex ← currenttextureindex`, `dtextures ← s`, `dtexturet ← t`, `texturesmax ← S`, `texturetmax ← T` |
| scalars | `nlimit = floor(n slider)`, `dwheel = speed × whl` then `wheel = fmod(wheel − dwheel, 360)`, `bhue += speed × outr × 0.05` (wrapped to [0,1)), `ghue` wrapped to [0,1) |
| background | `back` on → RGB background = hls(bhue, 0.5); off → black. Alpha = `baka` |
| spin/flip | only when the pre-wrap `t ≥ 1` (a new segment): `gspin = fmod(gspin + spn, 360)`, `gflip = fmod(gflip + flp, 360)` → `spin[n]`, `flip[n]` |
| draw | smear off → clear, `drawit`, swap. Smear on → no clear (fade first, CI only). With smooth also on: `drawit`, swap, `pnl_drawpanel`, then the normal `drawit`, swap. |

The renderer reads only `lightbutton`, `ribbonbutton`, `zbufbutton`, `speedslider`
(`drawit`/`drawribbons`) and `trianglebutton` (`drawshape`) directly. Everything else reaches it
through the ring buffers above.

---

## 8. Startup state summary

| control | ep with no options / `-b` | `-c` | `-S` |
|---|---|---|---|
| panels visible | no, but they appear if you leave full-screen | **yes** (all four) | no |
| window | regular at first; the script's `actset: 29, 1.0` then makes it full-screen | regular | borderless, unpositioned (UNCERTAIN) |
| script | built-in "mello" script running | none | built-in script running |
| fill | on | on | on |
| outline | on (set by the script; animates 0.2–1.0, so it toggles occasionally) | off | on |
| fullbutton | 1 (from the script) | 0 | 1 (script), but the window is not resized |
| auto | 0 (`actset: 17, 0.0`). Script animation runs anyway because `aflag` is set. | 0 | 0 |
| every other toggle (fat, ribbons, smooth, smear, back, z-buf, light, stop, triangles) | off | off | off |
| ep sliders | §3.3 initial values, then script overrides: zoom 0.06, twst 60, sz 1.2, n 40, hue-0 rate 0 | §3.3 values; hue channel 0 drifts at rate 0.01 | as the default column |
| colors panel | dials 1, 1, 0.2; adial 1; hue 1, light 0.5; dwell (1,0,0,0,0); active channel 0; chanrate 0; hue mode | same | same |
| texture | "no texture" selected | same | same |
| `load_spectrum_map` | yes | yes | **no** |
| WINFREEZE/THAW | yes | yes | no |

In every mode, the only exits are ESC or "exit" (both via `cleanexit`: restore the colormap,
`exit(0)`), the usage error (`exit(1)`), and libpanel's fatal "out of space in pnl_table" (more
than 4096 ids, unreachable here).

---

## 9. Open questions

1. **UNCERTAIN:** the exact `-o` dump text for the roughly 290 NULL-labelled slots. It depends
   on IRIX libc's `%s` with NULL. Resolve by running `ep -o` on IRIX 5.3.
2. **UNCERTAIN:** whether pressing "reset", or an "R" twice, on the texture sliders corrupts
   the `texturet`/`textures`/`texturesmax`/`texturetmax` ring-buffer pointers and crashes. The
   code says it would. Test "reset" on IRIX.
3. **UNCERTAIN:** how `-S` gets its window geometry and how haven ends the process.
4. **UNCERTAIN:** `-t` labels taken from the `readdir` buffer, and the second `izoom` scale
   argument.
5. Unclear intent: the lightpalette shows colour indices 512–638, which ep never writes, so its
   appearance depends on the system colormap. The port can draw a grey or lightness ramp.
6. `setrange` (the rate slider's activefunc) has no net effect beyond re-clamping. Its value is
   only consumed by `animateslider`. This looks like leftover code, but the port should keep
   "rate" as a plain per-slider oscillation speed.
