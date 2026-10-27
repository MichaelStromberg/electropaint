# electropaint (IRIX 5.3 `ep`): animation and script specification

Source: Ghidra export `re/ghidra/ep-irix53/` (IrisGL, MIPS BE, O32 PIC, gp = 0x1000aee0).
Every function address below is the real body (stub + 0xc). The constants were checked
against the instruction stream (`re/tools/disasm.py`), not only against the decompiler output.
`re/tools/epscript.py` is an executable reference model of everything in sections 1–4
(parser, sequencer, actuator animator, slider self-oscillation, drand48). It **matches the
ground-truth IRIX trace over 3600 frames, to the trace's precision (9 significant digits; angles in tenths)** (§8; checker:
`re/tools/check_trace.py`). Use it to generate test vectors for the Rust port.

Notation: `f32` means IEEE single, `f64` means double. Where the original mixes precisions,
the formulas below say which one each step uses. This matters for bit-exact replay.

---

## 0. Summary

* The animation is purely frame-based. One pass of the `main` loop is one tick. Sequencer
  time (`currentFrame`), actuator interpolation and the strand phase (`t`, `n`) all advance per
  tick and never read the clock. `time()` is called exactly once, to seed `srand48`.
  The loop is paced only by `swapbuffers()`, which waits for vertical retrace
  (see §4.4).
* The built-in script (`-b`, the default) is one 2186-byte C string at `.rodata 0x100010d0`,
  reached through the pointer `defaultScript @ 0x10000410`. It has 9 sequences: an implicit
  seq 0 plus seqs 1–8, holding 54 commands in total.
* Each frame, the sequencer (`tasteQueue`) runs due script commands. Script commands set
  per-actuator random-walk parameters: lower/upper limits, a mean duration and a variance.
  `setacttargets` then picks a new uniform random target and an exponential-ish duration
  for every actuator that is enabled and idle, and `animateacts` moves each moving actuator
  linearly toward its target by a fixed per-frame step. Motion is piecewise-linear with no
  easing.
* Random numbers come only from `drand48`: one draw for each target, one or two for each
  duration (an evaluate-twice `MAX` macro bug), and one for each `randdelay:` command.

---

## 1. The ep script

### 1.1 Selection flags (`main @ 0x40eaec`, getopt string `"Scrabsqot"`)

| flag | effect |
|---|---|
| (none) | `.data` initial values: `aflag @0x10000004 = 1`, `bflag @0x10000008 = 1`. The internal script is used. |
| `-b` | `bflag++`, `aflag++`, `cflag = 0` (internal script) |
| `-a` | `aflag++`, `bflag = 0`, `cflag = 0` (script read from stdin) |
| `-c` | `aflag = bflag = 0`, `cflag++` (no script; controls visible) |
| `-o` | `oflag++`: prints parse trace, actuator table (`# %d: %s`), `proc:` trace per command, recordact output |

`readAnimation()` runs only `if (aflag)`, before `srand48` and **before `defpanel()`**. The
actuators do not exist yet at parse time, so the parse only stores ids.

The per-frame sequencer and actuator animator run `if (aflag || autobutton is on)` (§4.1).
With the default flags `aflag = 1`, so the sequencer always runs.

### 1.2 Lexing (`readAnimation @ 0x40c0d4`)

Script source:
* `bflag != 0`: walk the in-memory string `defaultScript`.
* `bflag == 0` (the `-a` case): allocate a 256-byte buffer (`pnl_alloc(0x100)`) and
  `scanf("%[^\n]\n", buf)` one line at a time from stdin until `scanf` returns EOF. The
  trailing `\n` in the format also swallows blank lines after a line. There is no length
  check, so lines of 256 characters or more overflow the buffer.

For each line, starting with `p` at the start of the line:
1. `sscanf(p, "%s", tok)` reads the first whitespace-delimited token, and
   `p += strcspn(p, " \t\n")`, which skips the token when the line does not begin with
   whitespace.
2. Allocate a 0x18-byte command (calloc'd). Set `cmd.frame = absFrame`. **The timestamp is
   taken before the current line's `frame:`/`absframe:` update is applied** (store at
   `0x40c228`).
3. Dispatch on `tok` with `strcmp` (table in §1.3). Arguments are read with `sscanf` from
   `p`, which still points just after the keyword. After the arguments, `p` is advanced by
   `strcspn(p," \t\n")`, which is 0 because the next character is a space. That is harmless.
4. The rest of the line is consumed by `sscanf(p, "%[^\n]\n", junk)` (echoed when `-o`).
   Then, for the internal script, `p += strcspn(p, "\n") + 1`. If `strlen(p) == 0`, parsing
   stops. The internal script has no trailing newline, so after the last line `p` points one
   byte past its NUL, at `0x1000195b`. That byte is also 0 (padding), so the loop stops
   cleanly. A port should simply stop at end of text.
5. A token beginning with `#` (`strncmp("#", tok, 1)`) is a comment. Unknown tokens print
   `bad command: %s` under `-o`. Both are freed without being added.

Edge cases a port may ignore: a blank line inside an in-memory script would make `%s` read
the next line's token, so that command would run twice. The internal script has no blank
or indented lines (verified).

### 1.3 Grammar and command codes

```
script   := { line '\n' }
line     := '#' anything
          | keyword args [ '#' comment ]
keyword args:
  frame:      <float>          code 0     relFrame += f; absFrame = relFrame + baseFrame
  relframe:   <float>          code 0     (identical to frame:)
  absframe:   <float>          code 1     relFrame = 0; baseFrame = absFrame = f
  actlim1:    <int>, <float>   code 2     sscanf "%d, %f"
  actlim2:    <int>, <float>   code 3
  actset:     <int>, <float>   code 4
  actreset:   <int>            code 5
  actresetall:                 code 6
  actstop:    <int>            code 7
  actstopall:                  code 8
  seqname:    <int>            (code 9 assigned, but never stored; see below)
  seqdo:      <int>            code 10
  seqstop:    <int>            code 11
  seqloop:                     code 12
  seqrepeat:  <int>            (code 13; parsed then freed: "warning: %s not implemented")
  seqkill:    <int>            code 14
  seqkillall:                  code 15
  duration:   <float>          code 0x65 (101)
  variance:   <float>          code 0x66 (102)
  randdelay:  <float>          code 0x67 (103)
```

The integer field (`cmd.iarg`, +0x4) holds the actuator id or sequence id. The float field
(`cmd.farg`, +0xc) holds the value. Fields the command does not use stay 0 (calloc).

**Sequence building.** The globals are `editSeq @0x10003950` (bss, initially NULL),
`seqList @0x100000f0`, `relFrame @0x100000e4`, `baseFrame @0x100000e8` and
`absFrame @0x100000ec`, all initially 0.
* `seqname: N` resets `baseFrame = absFrame = relFrame = 0`. It then searches `seqList` for
  id N. If found, `editSeq` becomes that sequence; otherwise a new sequence (defaults below)
  is pushed onto the **front** of `seqList`. The command object is freed.
* Any other stored command: if `editSeq == NULL`, a new sequence with **id 0** is created and
  pushed to the front. Then `addToSeq(editSeq, cmd)` runs and `editSeq.looplen = relFrame`
  (stored as f32, `0x40d05c`).
* New sequence defaults (`0x40cf60`, `0x40cae8`): `active = 1`, `start = 0`,
  `duration = 300.0` (`0x43960000`; earlier versions of this document said 150), `variance = 1.0`, `head = cur = tail = NULL`,
  `looplen = 0`.

**`addToSeq @ 0x40bff4`** is a stable insertion sort on `cmd.frame`. The new command goes
before the first existing command with a strictly greater frame. Ties keep file order.
It sets `cmd.seq = seq`.

**End of parse:** walk `seqList` from the head and stop at the first sequence with
`id == 0`, setting its `cur = head`. **Only seq 0 starts automatically.** Every other
sequence keeps `cur = NULL` until a `seqdo:`. A script with no command before its first
`seqname:` has no seq 0, so nothing ever runs unless it defines `seqname: 0` itself.
With `-o`, each sequence is printed as `seq %d at %.2f` (its `start`) followed by
`  cmd %d,\t%d\t%f` lines.

### 1.4 The internal script (complete parsed content)

Raw text (2186 bytes at `0x100010d0`, author Dave Tristram, "mello script"). Every
command in it has `cmd.frame = 0`, because there are no `frame:` lines, and every
`looplen = 0`. `seqList` order (head first) is **8, 7, 6, 5, 4, 3, 2, 1, 0**. This order
matters for random-number consumption.

The actuator names come from the comments in the script and from the
`defpanel @ 0x414958` creation order (§5.3).

| seq | # | cmd (code) | iarg | farg | meaning |
|---|---|---|---|---|---|
| 0 | 1 | actset (4) | 29 | 1.0 | "full screen" toggle on (calls `setfullscreen`) |
| 0 | 2 | actset (4) | 17 | 0.0 | "auto" button off, so `setauto` calls `stopAnim` |
| 0 | 3 | actset (4) | 18 | 1.0 | "outline" toggle on |
| 0 | 4 | actset (4) | 31 | 0.06 | zoom slider |
| 0 | 5 | actset (4) | 38 | 60.0 | twist slider |
| 0 | 6 | actset (4) | 108 | 1.2 | size slider |
| 0 | 7 | actset (4) | 52 | 40.0 | n slider (strand length, so `nlimit = 40`) |
| 0 | 8–15 | seqdo (10) | 1…8 | – | start seqs 1,2,3,4,5,6,7,8 (in that order) |
| 1 wrist | 1 | duration | – | 160 | |
| 1 | 2 | actlim1 | 101 | -1.5 | wrist slider |
| 1 | 3 | actlim2 | 101 | 1.5 | |
| 2 color | 1 | duration | – | 60 | |
| 2 | 2 | actset | 127 | 0.0 | hue chan 0 *rate* sub-slider, turning off its self-oscillation |
| 2 | 3 | actlim1 | 123 | 0.544053 | hue chan 0 ("just near cyan") |
| 2 | 4 | actlim2 | 123 | 1.295 | ("deep green") |
| 2 | 5 | duration | – | 80 | |
| 2 | 6 | actlim1 | 130 | 0.0 | lightness chan 0 |
| 2 | 7 | actlim2 | 130 | 1.0 | |
| 3 wheel | 1 | duration | – | 120 | |
| 3 | 2 | actlim1 | 73 | 0.137 | wheel slider |
| 3 | 3 | actlim2 | 73 | -0.137 | |
| 4 spin | 1 | duration | – | 100 | |
| 4 | 2 | actlim1 | 80 | 0.23 | spin slider, slow |
| 4 | 3 | actlim2 | 80 | -0.23 | |
| 4 | 4 | randdelay | – | 1000 | |
| 4 | 5 | duration | – | 40 | |
| 4 | 6 | actlim1 | 80 | 5.23 | spin, fast |
| 4 | 7 | actlim2 | 80 | -5.23 | |
| 4 | 8 | randdelay | – | 200 | |
| 4 | 9 | seqloop (12) | – | – | |
| 5 flip | 1 | duration | – | 50 | (also applies to the fast phase) |
| 5 | 2 | actlim1 | 87 | 2.0 | flip slider, slow |
| 5 | 3 | actlim2 | 87 | -2.0 | |
| 5 | 4 | randdelay | – | 1200 | |
| 5 | 5 | actlim1 | 87 | 10.0 | flip, fast |
| 5 | 6 | actlim2 | 87 | -10.0 | |
| 5 | 7 | randdelay | – | 220 | |
| 5 | 8 | seqloop | – | – | |
| 6 arm | 1 | duration | – | 90 | |
| 6 | 2 | actlim1 | 94 | -2.0 | arm slider |
| 6 | 3 | actlim2 | 94 | 2.0 | |
| 7 twist | 1 | duration | – | 2250 | |
| 7 | 2 | actlim1 | 38 | 200.0 | twist slider |
| 7 | 3 | actlim2 | 38 | -200.0 | |
| 8 outline | 1 | duration | – | 5000 | |
| 8 | 2 | actlim1 | 18 | 0.2 | outline toggle (on when val > 0.5) |
| 8 | 3 | actlim2 | 18 | 1.0 | |

Commented-out lines in the source (`# actset: 73, 0.107258`, `#actset: 80, 0.23`,
`#randdelay: 100`, `#randdelay: 500`, `#randdelay: 80`, `#duration: 1750`) are comments
and produce nothing. Variance is never set, so every sequence keeps `variance = 1.0`.

Resulting actuator random-walk parameters after the first pass (after the clamps in §2.3,
none of which bind for these values):

| act | name | lower | upper | duration | slider range [min,max] | initial val |
|---|---|---|---|---|---|---|
| 18 | outline | 0.2 | 1.0 | 5000 | [0,1] toggle | 1.0 (actset) |
| 38 | twist | -200 | 200 | 2250 | [-200,200] | 60 (actset) |
| 73 | wheel | -0.137 | 0.137 | 120 | [-60,60] | 0 |
| 80 | spin | -0.23 / -5.23 | 0.23 / 5.23 | 100 / 40 | [-20,20] | 0 |
| 87 | flip | -2 / -10 | 2 / 10 | 50 / 50 | [-20,20] | 0 |
| 94 | arm | -2 | 2 | 90 | [-3,3] | 0 |
| 101 | wrist | -1.5 | 1.5 | 160 | [-3,3] | 0 |
| 123 | hue ch0 | 0.544053 | 1.295 | 60 | [0,2], wraps | 1.0 |
| 130 | light ch0 | 0.0 | 1.0 | 80 | [0,1], wraps | 0.5 |

---

## 2. Sequencer

### 2.1 Data

Sequence (0x28 bytes, calloc):

| off | field | notes |
|---|---|---|
| +0x00 | `id` (int) | |
| +0x04 | `active` (int) | 1 at creation; only `killSeq` clears it |
| +0x08 | `start` (f32) | frame origin; command times are relative to it |
| +0x0c | `looplen` (f32) | `relFrame` after the last command added; `seqloop` adds it to `start` |
| +0x10 | `duration` (f32) | default 300.0, copied into an actuator by `actlim2` |
| +0x14 | `variance` (f32) | default 1.0, clamped to [0,1] |
| +0x18 | `head` | command list |
| +0x1c | `cur` | next command to run; NULL means stopped |
| +0x20 | `tail` | |
| +0x24 | `next` | `seqList` link |

Command (0x18 bytes): `+0 code`, `+4 iarg`, `+8 frame (f32)`, `+0xc farg (f32)`,
`+0x10 seq` (back pointer), `+0x14 next`.

Globals: `currentFrame @0x100000e0` (f32, initially 0), and `autoReleased @0x100000f8`
(int, initially 0; Ghidra `DAT_100000f8`). `seqId @0x100000dc` exists in `.data` but no
code in the analysed paths reads it.

### 2.2 `tasteQueue @ 0x40e20c`, once per frame

```
currentFrame = f32(currentFrame + 1.0)                 // 0x40e228
busy = false
for s in seqList (head → tail):                        // order 8,7,…,1,0 for the internal script
    if s.active && s.cur: busy = true                  // tested BEFORE running s's commands
    now = f64(currentFrame) + 0.5                       // 0x3fe00000 @0x40e254
    if f64(s.start) < now && s.active && s.cur
       && f64(f32(s.cur.frame + s.start)) < now:
        loop:
            processCommand(s.cur)
            if !s.active || !s.cur: break
            if !(f64(f32(s.cur.frame + s.start)) < f64(currentFrame) + 0.5): break
if !autoReleased && !busy:
    autobutton.val = autobutton.minval  (= 0, which switches "auto" OFF); pnl_fixact
    autoReleased = 1
```

Timing: a command is due on the first tick F (F = 1, 2, …) with `start + frame < F + 0.5`.
All due commands of one sequence run in the same tick, in list order. Sequences are visited
in `seqList` order. `seqdo` sets the target's `start = currentFrame`. If the target comes
after the caller in the list, its first commands run in the same tick. If it comes earlier
(as seqs 1–8 do relative to seq 0), they run on the **next** tick.

For the internal script: tick 1 runs all of seq 0. Tick 2 runs seqs 8…1 up to their first
`randdelay` (seqs 4 and 5) or to their end (the others). From then on only seqs 4 and 5
ever fire again. `busy` stays true forever, so the auto-release never happens.

**UNCERTAIN (cosmetic):** the auto-release sets the auto button to its *minval* (off)
without calling `setauto`. It never runs with the internal script. Under `-c` with auto
toggled on by the user it would switch auto back off on the first frame. Confirming this
needs a runtime check under `-c`.

### 2.3 `processCommand @ 0x40da04` (argument `cmd` in a0; `s = cmd.seq`; `adv` means `s.cur = s.cur.next`)

`rec = acttable[cmd.iarg]` (see §3). "if rec" means skip when the slot is NULL.

| code | effect | advances `s.cur`? |
|---|---|---|
| 0 frame/relframe | nothing (time marker only) | yes |
| 1 absframe | nothing | yes |
| 2 actlim1 | if rec: `rec.lower = (farg < act.minval) ? act.minval : farg` | yes |
| 3 actlim2 | if rec: if `rec.lower < farg`: `rec.upper = (act.maxval < farg) ? act.maxval : farg` (lower unchanged); else `rec.upper = (act.maxval < rec.lower) ? act.maxval : rec.lower` and `rec.lower = (farg < act.minval) ? act.minval : farg`. Then `rec.duration = s.duration`, `rec.variance = s.variance`, `rec.active = 1`. All compares are f32. | yes |
| 4 actset | if rec: `setact(act, farg)` (§3.4) | yes |
| 5 actreset | if rec: `act.val = act.initval`; `pnl_fixact`. For a multislider this does **not** move the value bar (only `val`) | yes |
| 6 actresetall | `reset()` (§3.6) | yes |
| 7 actstop | if rec: `rec.active = 0` (`moving` is left as is) | yes |
| 8 actstopall | `stopAnim()`: every rec's `active = 0` | yes |
| 9 | nothing (never stored) | yes |
| 10 seqdo N | t = first seq with id N. If t: `t.cur = t.head; t.start = currentFrame`. If `t != s`: adv. If t is s (self-restart), no adv; cur is already head | conditional |
| 11 seqstop N | t = find(N); if t: `t.cur = NULL`. If `t != s`: adv | conditional |
| 12 seqloop | `s.cur = s.head; s.start = f32(s.start + s.looplen)` | n/a |
| 13 | adv only (never stored) | yes |
| 14 seqkill N | t = find(N). **Bug:** acts only if `t == s` (`killSeq(s)`). If `t != s` it returns without advancing, so `tasteQueue` spins forever on this command. | – |
| 15 seqkillall | **Bug:** if the list head is not s, it kills the head, then prints "bad command type" and advances. If the head is s, it advances `s.cur` once for each leading entry equal to s… This path is broken; do not rely on it. | – |
| 0x65 duration | `s.duration = (f64(farg) < 1.0) ? 1.0 : farg` | yes |
| 0x66 variance | `s.variance = farg < 0 ? 0 : min(farg, 1.0)` | yes |
| 0x67 randdelay | `e = exprand(1.0)`; `s.start = f32( f64(s.start) + ( f64(var)*((f64(e)+f64(e))*f64(d)) + (1.0-f64(var))*f64(d) ) )` with `d = farg`, `var = s.variance` (`0x40db34`) | yes |
| other | `printf("warning: bad command type (%d)…")`, then adv | yes |

Notes on code 2 and code 3: after an actlim1/actlim2 pair, `upper = min(max(l1,l2), maxval)`
and `lower = max(min(l1,l2), minval)`. So `upper` (+0x14) is the larger limit and `lower`
(+0x18) the smaller, whatever order the two limits were given in. Ghidra's `(int)` casts on
these stores are wrong; they are plain `swc1` float stores.

`actlim2` sets new limits and duration but does **not** reset `moving`, `target` or `step`.
A change of range (seq 4/5 switching slow↔fast) takes effect at that actuator's next
retarget, not immediately.

`killSeq @ 0x40d6e8`: sets `seq.active = 0` and unlinks and frees the sequence and its
commands. **Bug:** when the victim is the list head, it frees the *new* head's command list
instead of its own. A port should implement the intended unlink.

With the default variance of 1, `randdelay: d` waits `2·e·d` frames, where e = exprand (see §3.2).
The mean is `0.836·d` and the range is [0, 2d).

### 2.4 Behaviour of the internal script over time

* Seq 4 (spin) alternates between a slow phase of `2e·1000` frames (range ±0.23, duration 100)
  and a fast phase of `2e·200` frames (±5.23, duration 40).
* Seq 5 (flip) alternates between slow `2e·1200` (±2) and fast `2e·220` (±10), both with
  duration 50.
* All other actuators keep their limits forever after tick 2.

---

## 3. Actuator animation

### 3.1 Actuator record (`acttable`, 0x28 bytes, made by `ep_mkact @ 0x41432c`)

`acttable @0x10003988` holds 1024 pointers, indexed by libpanel actuator id.
`acttablemaxindex @0x10000054` is the largest id passed through `ep_mkact`: 214 when
`has_textures == 0` (the `chanrateslider`). When the hardware reports textures, the texture
panel is built whether or not `-t` is given (`-t` only loads `./*.rgb`), so the maximum goes
up to 345 (the 4 texture sliders at 318–345; the trace machine's `-o` table lists ids up to
345). This never affects the script ids.

| off | field | |
|---|---|---|
| +0x00 | `act` | the libpanel Actuator |
| +0x04 | `active` | enabled for random animation (set by actlim2, cleared by actstop/stopAnim) |
| +0x08 | `moving` | 1 while travelling to `target` |
| +0x0c | `dir` (f32) | +1.0 or -1.0 |
| +0x10 | `target` (f32) | |
| +0x14 | `upper` (f32) | |
| +0x18 | `lower` (f32) | |
| +0x1c | `step` (f32) | per-frame increment |
| +0x20 | `duration` (f32) | mean-ish segment length in frames |
| +0x24 | `variance` (f32) | 0…1 |

### 3.2 `exprand(m) @ 0x409364`

```
assert(m > 0.0)                          // "m > 0.0", "ep.c", line 0x101
u = drand48()                            // f64
return f32( expm1(u * f64(m)) / expm1(f64(m)) )
```
It is always called with m = 1.0f. The result E lies in [0,1) with CDF `P(E≤x) = ln(1 + x(e−1))`
and mean `(e−2)/(e−1) = 0.418023`. So `2E` lies in [0,2) with mean 0.836.

### 3.3 `setacttargets @ 0x40e3d8`, once per frame

```
for i in 0 .. acttablemaxindex-1:              // NOTE: the max index itself is never visited
    r = acttable[i]
    if r && r.active && !r.moving:
        r.moving = 1
        u  = f32(drand48())                                      // draw #1
        r.target = f32( f64(f32(r.upper * u)) + (1.0 - f64(u)) * f64(r.lower) )
        e1 = exprand(1.0)                                        // draw #2
        D  = (f64(e1)+f64(e1)) * f64(r.duration) * f64(r.variance)
             + (1.0 - f64(r.variance)) * f64(r.duration)
        if D < 1.0:  D = 1.0
        else:                                                    // MAX() macro evaluates twice
            e2 = exprand(1.0)                                    // draw #3
            D  = (f64(e2)+f64(e2)) * f64(r.duration) * f64(r.variance)
                 + (1.0 - f64(r.variance)) * f64(r.duration)
        r.step = f32( f64(f32(r.target - r.act.val)) / D )
        r.dir  = (0.0 < f64(r.step)) ? 1.0 : -1.0
```
The order of operations above matches the instruction stream (`0x40e498`–`0x40e5cc`).
The IRIX 6.5 build also evaluates exprand twice (`1000dd98`), which confirms the
evaluate-twice behaviour is in the source (a `MAX` macro). The duration used is the second
draw, not the first. With variance 1 and durations of 40 or more, the first estimate is below 1
only when `e1 < 1/(2·dur)`, so almost every retarget uses **3 draws**.

The target is uniform in [lower, upper]. The segment length is `D = 2E·duration` frames
when variance is 1, or `duration` when variance is 0.

### 3.4 `animateacts @ 0x40e630`, once per frame after `setacttargets`

```
for i in 0 .. acttablemaxindex-1:
    r = acttable[i]
    if r && r.active && r.moving:
        v = f32(r.act.val + r.step)
        if f64(f32(f32(r.target - v) * r.dir)) < 0.0:   // overshot
            r.moving = 0
            v = r.target
        setact(r.act, v)
```
Each segment moves linearly at a constant step and lands exactly on `target` on the
overshoot frame. On the next frame `setacttargets` picks a new target and that frame
already moves, so there are no idle frames.

Quirks that a bit-exact port reproduces automatically with f32 arithmetic:
* If `step` is so small that `val + step == val` in f32 (for example twist near ±200 with
  `|target−val|` below about `D·ulp(val)/2`), the actuator never arrives and freezes for the rest of
  the run. The same happens when `target == val` exactly (`step = 0`, `dir = −1`,
  `(target − v)·dir = −0.0`, which is not < 0).
* `currentFrame` is f32. After 2^24 ticks (about 77.7 h at 60 Hz), `currentFrame + 1 == currentFrame`,
  so seqs 4 and 5 stop advancing. Actuators keep walking.

### 3.5 `setact(act, v) @ 0x40d824`

```
if act.type == 0x5a:                        // ep multislider (pnl_vmultislider)
    d = act.u                               // ep slider data (+0xa0)
    A = d.topbar.extval;  B = d.botbar.extval
    x = v clamped into [min(A,B), max(A,B)]  // if B<A: x<B→B, x>A→A ; else x<A→A, x>B→B
    d.valuebar.extval = x;  act.val = x
elif act.type == 2:                         // pnl_filled_slider (colour dwell sliders)
    act.val = v;  chooseupdates(act)
else:
    act.val = v
if act.downfunc:   act.downfunc(act, act.p)      // +0x90
if act.activefunc: act.activefunc(act, act.p)    // +0x94
if act.upfunc:     act.upfunc(act, act.p)        // +0x98
pnl_fixact(act)                                  // redraw bookkeeping only
```

Callbacks reachable from the internal script:
* id 29 `fullbutton` (toggle 0x137): downfunc `setfullscreen` (window management).
* id 17 `autobutton` (wide button 0x139): upfunc `setauto @0x40d654`. If
  `val < 0.5·maxval + 0.5·minval`, it calls `stopAnim()` (clears every rec's `active`).
* id 18 `outlinebutton` (toggle): no callbacks.
* id 127, the rate hslider of hue ch0 (type 1): downfunc `saverange`, activefunc `setrange`.
  Together they recompute `range/center` and re-clamp the open bars and value bar. That is
  a numerical no-op here; the only effect is `rate = 0`.
* The multisliders (31, 38, 52, 73, 80, 87, 94, 101, 108, 123, 130) have no callbacks.

`chooseupdates(a) @0x409b2c`: for `i` in 0..4, if `a == dwellslider[i]` it calls
`updatedwellsliders(i)`, and if `a == activebutton[i]` it calls `updateactivebutton(i)`.
* `updatedwellsliders(i) @0x4094a0`: the other four dwell values are renormalised. Let
  `S = Σ_{j≠i} val_j` and `δ = dwell[i].val − prev[i]`, where `prev` is `DAT_10004b88[5]`,
  the snapshot from the last call. If `S == 0`, each other `val_j −= 0.2·δ`; otherwise
  `val_j −= δ·val_j/S`. Each is clamped to [0,1], then all five are snapshotted into `prev`.
* `updateactivebutton(i) @0x4096d8`: every other `activebutton[j].val = minval`.
* Neither is reached by the internal script, because no script id is a dwell slider or
  active button.

### 3.6 Slider self-oscillation: `animatesliders @0x40bfa4` / `animateslider @0x40bc8c`

`animatesliders` runs every frame, whatever the auto/script gate says. It walks
`modsliderlist`, a push-front list built by `mkepslider`, so the order is reverse creation:
texture sliders, then `lslider[4], hslider[4], …, lslider[0], hslider[0]`, then size, wrist,
arm, flip, spin, wheel, (shiny|maprange), (ambient|maprate), n, speed, twist, zoom.

```
animateslider(ms):
    d = ms.u
    rate = d.rateslider.val
    if pnl_ca == d.valuebar: return           // user is dragging
    if rate == 0.0: return
    A = d.topbar.extval;  B = d.botbar.extval
    delta = f32( f32( f32(f32(d.dir * rate) * f32(A - B)) / f32(ms.maxval - ms.minval) ) * speed )
    bar = f32(d.valuebar.extval + delta)
    if f64(d.bouncetoggle.val) > 0.5*maxval_t + 0.5*minval_t:     // bounce mode
        if bar < B:   bar = 2B − bar; d.dir = +1
        elif A < bar: bar = 2A − bar; d.dir = −1
    else:                                                          // wrap mode
        if bar < B:   bar = A − (B − bar)
        elif A < bar: bar = B + (bar − A)
    bar = clamp into [min(A,B), max(A,B)]
    d.valuebar.extval = bar;  ms.val = bar
```
`speed` is `speedslider`'s value bar extval, read live: sliders processed before the speed
slider in the list see the speed value from before that slider's own update.

In the default run every rate slider is 0. The only exception is `hslider[0]`'s rate, which
starts at 0.01 (`0x3c23d70a`, set in defpanel) and is zeroed by `actset: 127, 0.0` on
**tick 2**. So on tick 1 the hue ch0 value bar moves once by
`f32(f32(f32(1·0.01)·2.0)/2.0)·1.0 = 0.01` in wrap mode, from 1.0 to 1.01. The first
`setacttargets` (tick 2) reads that value. After tick 2, `animateslider` does nothing in the
default run. The trace confirms this step (§8): without it the colour of every frame is off
by 0.01 in hue.

**UNCERTAIN:** the `pnl_fixact()` at the end of `animateslider` has no argument in the
decompilation. It is presumably `pnl_fixact(ms)` and has only visual effect.

### 3.7 Range helpers (user interaction or `setact` on sub-actuators)

* `saverange(a) @0x40b640`: `d = a.pa.u; d.range = A − B; d.center = A − 0.5·range`.
* `setrange(a) @0x40b894`: `A = 2·center − B`, then clamp A into the span between B and the
  parent's `maxval`; clamp the value bar into [A,B]; `saverange`; `pnl_fixact(parent)`.
* `setcenter(a) @0x40b69c`: `A = B + range`, then the same clamps.
* `resetslider(btn) @0x40ad9c`: `A = maxval; B = minval; rate = rate.initval`. Then, if the
  value bar extval already equals `initval`, `*btn.ptr = btn.initcopy` (resets the external
  accumulator such as `wheel`, `gspin` or `gflip`); otherwise the value bar extval is set to
  `initval`.
* `resetact(btn) @0x40ad14`: the same two-step behaviour for dials.
* `reset() @0x40aecc` (cmd 6, "reset" button): for every entry of `rblist`, call its downfunc
  **twice**, so the value goes back to init and then the accumulator is reset. Then
  `dwheel = 0`, puck x/y = 0, and every button or dial whose `p != NULL` gets
  `val = initval` (smear, zbuf, smooth, outline, fill, fatline, fade, ribbon, background,
  alpha dials, hue rate dials, light, `adial[i]`, `dwellslider[i]`, chanrate, palettes).
  Finally it calls `setsmear/setzbuf/setsmooth/togglechanmode`. **The acttable recs are not
  touched**, so actuators keep walking.

### 3.8 `recordact @0x40d280` (panel activefunc/upfunc; interactive only)

When the user presses an actuator and auto is on, `rec.lower = act.val`. Under `-o` it
prints `actlim1: %d, %f\t# "label"`. On release, under `-o` with auto on, it prints
`actlim2:` and sets `upper/lower` the same way `actlim2` does. Then `rec.duration` is set
(from `editSeq.duration`, or 300.0 if there is no editSeq), `rec.variance` is forced to
**1.0** (the editSeq value is overwritten), and `rec.active = 1`. It also emits
`relframe: %d` lines when more than one `pnl_frame_number` tick has passed. Screensaver
mode never calls it.

### 3.9 Random-number usage: complete list and per-frame order

| call site | function | draws |
|---|---|---|
| `main` 0x40eed4 / 0x40eeec / 0x40ef04 | `srand48(0)`, `time(NULL)`, `srand48(time)` | seeding only; the only wall-clock read |
| `processCommand` code 0x67 | `exprand` | 1 per `randdelay` executed |
| `setacttargets` | `drand48` + `exprand` (+ `exprand`) | 2, or 3 when the first duration ≥ 1 |

There are no other `drand48`, `random` or `rand` callers in the binary. Checked: the only
importers are `exprand`, `setacttargets` and `main`.

Order within one frame:
1. `tasteQueue`: for each sequence in `seqList` order (head first), each executed
   `randdelay`, in command order.
2. `setacttargets`: for actuator ids ascending (0 … maxindex−1), each enabled idle record.
3. `animateacts` and `animatesliders` make no draws.

For the internal script: ticks 1 and 2 make 0 and 29 draws (2 randdelays, then 9 actuators
× 3 in the order 18, 38, 73, 80, 87, 94, 101, 123, 130). After that, draws happen only on
arrivals and when seqs 4 or 5 fire.

`srand48(seed)`/`drand48()` follow the SVID definition:
`X0 = (seed32 << 16) | 0x330E`, `X ← (0x5DEECE66D·X + 0xB) mod 2^48`, return `X / 2^48`.
This is confirmed bit-exact against IRIX (`re/fixtures/d48_<seed>.txt`).

Reference: with `time() = 12345`, tick 2 consumes **28** draws, not 29. The flip record's
first duration estimate is `2·exprand·50 < 1` (u = 0.00434), so it takes the 2-draw path, and
`D = 1` makes flip jump to its first target (−1.4921) in a single frame. The run consumes
1160 draws in total over 3598 ticks.

---

## 4. Main loop timing (`main @ 0x40eaec`)

### 4.1 Per-frame order

Initialisation before the loop: `n = 0`, `t = 0.0`, `gchan = 0`, `gchant = 0`,
`gcol = ncolors/2 + 0x280`, `bhue = ghue = 0.5`, `wheel = 0`, `pnl_naptime = −1`.
The negative naptime means `pnl_dopanel` never calls `sginap`.

```
loop:
  pnl_dopanel()                                    // UI/events; in saver mode it changes nothing here
  first iteration only, if -r: fullscreen(); pnl_beginreadscript()
  if stop button on: clear if !smear; drawit; swapbuffers;
                     spin on { pnl_dopanel(); if no action: sginap(0); swapbuffers() } while stop on
  if (aflag || autobutton.val > 0.5*(max+min)):   // 0x40faf0
      tasteQueue(); setacttargets(); animateacts();
  animatesliders()                                  // always
  t = f32(t + speed)                                // speed = speedslider value bar (init 1.0, range [0,1])
  crossed = (1.0 <= f64(t))
  n = trunc(f64(n) + floor(f64(t)))                 // cvt.w.d with the FCSR rounding mode forced to RZ
  t = f32(fmod(f64(t), 1.0))
  if rgb_mode: gchant = f32(fmod(gchant + chanrateslider.val*speed, 1.0))
  k = n mod 128 (C '%' semantics, toward zero, then +128 if negative)
  ring[k] ← current values: x,y (positionpuck), dzoom←zoom, arm, wrist, dtwist←twist,
            outline←(outlinebutton.val > mid), fatline, fill, size,
            (CI: maprange) | (RGB: alpha←adial[gchan], alphaout, shiny, ambient),
            textureindex, dtextures, dtexturet, texturesmax, texturetmax
  dwheel = f32(speed * wheel_slider)
  nlimit = trunc_f32→int(nslider value)            // 40 after the script
  CI: gcol += speed*maprate ; RGB: ghue = hslider[gchan] value; glight = lslider[gchan] value;
      bhue += speed*huebackratedial.val*0.05
  wrap gcol into [basecolor, basecolor+ncolors) or ghue,bhue into [0,1)
  RGB: hue[k]=ghue; light[k]=glight; pick gchan (activebutton held, else cumulative dwell
       vs gchant); mapcolor(0x300+i, hls_to_rgb(fmod(hue_i,1), light_i)) for i in 0..4
  background colour (bkg / bkgRGBA from gcol or bhue; alpha from alphabackdial)
  if crossed:                                      // 0x410984
      dflip = flip slider; dspin = spin slider
      gflip = f32(fmod(gflip + dflip, 360)); flip[k] = gflip
      gspin = f32(fmod(gspin + dspin, 360)); spin[k] = gspin
  window events (REDRAW → reshape/perspective; INPUTCHANGE)
  wheel = f32(fmod(wheel − dwheel, 360))
  if !smear: clear
  else: if fade: fadebackground(bkg,1)
        if smooth: drawit; swapbuffers; if fade: fadebackground(bkg,0); pnl_drawpanel
  drawit(); swapbuffers()
```

### 4.2 What one frame advances

* Sequencer clock: `currentFrame += 1`, independent of speed.
* Actuator walks: one `step` per frame, independent of speed. Segment lengths are measured
  in frames.
* Strand phase: `t += speed`. On each whole-number crossing, `n` advances by the number of
  units crossed. At the default speed of 1.0, `n` advances by exactly 1 every frame and
  `crossed` is always true. If `speed > 1`, `n` can skip slots, and those slots keep stale
  values. With `speed < 1`, slot `n` is rewritten every frame until `n` advances, and flip and
  spin only accumulate on the crossing frames.
* `wheel`, `gcol`, `bhue` and `gchant` advance by `speed ×` their rate every frame.
* Ring buffers: 128 entries (each `pnl_alloc(0x200)`). `nlimit` (the n slider, 40 after
  the script) is how many history entries the renderer uses.

### 4.3 Wall clock

`time()` is called once at `main` (srand48 seed). `sginap()` is called only in
`pnl_dopanel` when `pnl_naptime >= 0`, which is never true because main sets −1, and as
`sginap(0)` inside the "stop" idle loop. No `gettimeofday`, `times` or `clock` is imported.
**Animation is purely frame-based.**

### 4.4 Frame rate

**UNCERTAIN:** the real-time speed depends on how often `swapbuffers()` returns. IrisGL
swaps at vertical retrace, so 60 Hz on typical Indy/Indigo2 setups, 72/76 Hz on some
monitors, or lower if `drawit` takes longer than one refresh. In smooth mode two swaps
happen per tick, which halves the rate. Recommended port behaviour: a fixed 60 Hz tick
(one tick per frame at 60 Hz vsync, with a fixed-timestep accumulator otherwise).
Measuring the frame rate on real hardware (frames per second at the default settings)
would settle this.

---

## 5. libpanel structures relied on

### 5.1 Actuator (0xc4 bytes, `pnl_mkact @0x418808`)

The layout comes from the `pnl_dumppanel` format strings (`a->val`, `a->maxval`, …,
`strings.tsv` 0x1000203c–0x100023c0) and from how the fields are used.

| off | field | default (pnl_mkact) |
|---|---|---|
| +0x00 | `short id` (= index in `pnl_table`/`acttable`) | `pnl_id++` |
| +0x04 | `Panel *p` | set by `pnl_addact` |
| +0x08 | `Actuator *pa` (parent; ep also stores the parent multislider here for the rate hslider) | 0 |
| +0x10 | child list head (`ca`); siblings linked by +0xc0 | 0 |
| +0x18 | `int type` | set by the init function |
| +0x1c | `active` | 0 |
| +0x20/+0x24/+0x28/+0x2c | x, y, w, h | |
| +0x30…+0x40 | lx, ly, lw, lh, ld | |
| +0x44 | `float val` | 0.0 |
| +0x48 | `float extval` (bar position for multislider sub-bars) | 0 |
| +0x4c | `float initval` (`pnl_addact` copies `val` here) | |
| +0x50 | `float maxval` | **1.0** |
| +0x54 | `float minval` | **0.0** |
| +0x58 | float, initialised to 1.0 (scale factor?) | 1.0 |
| +0x5c | `char *label` | |
| +0x60 | `short key` | |
| +0x64 | `labeltype` | |
| +0x68 | init function | |
| +0x6c addfunc, +0x70 addsubfunc, +0x74 fixfunc, +0x78 pickfunc, +0x80 newvalfunc, +0x84 dumpfunc, +0x88 loadfunc, +0x8c drawfunc | | |
| +0x90 | downfunc | 0 |
| +0x94 | activefunc | 0 |
| +0x98 | upfunc | 0 |
| +0x9c | dirtycnt | |
| +0xa0 | `u` (user data: the ep slider struct for multisliders) | 0 |
| +0xa4 | `data` (libpanel private) | |
| +0xac/+0xb0/+0xb4 | automatic / selectable / visible | 0/1/1 |
| +0xbc | group | self |
| +0xc0 | next | |

A toggle is "on" when `val > 0.5·maxval + 0.5·minval`, computed in f64 everywhere.

Type codes: hslider 1, filled_slider 2, dial 0x50, vmultislider 0x5a, button 0x136,
toggle 0x137, wide button 0x139.

### 5.2 ep slider (`mkepslider @0x414488`, `u` = calloc 0x24)

Seven libpanel ids are allocated per slider, in this order: N multislider, N+1 top open bar,
N+2 bottom open bar, N+3 value bar, N+4 rate hslider, N+5 bounce toggle, N+6 "R" reset wide
button (useract, 0xd4 bytes).

| off | field | init |
|---|---|---|
| +0x04 | center (f32) | 0 (set by saverange) |
| +0x08 | range (f32) | 0 |
| +0x0c | dir (f32, ±1) | 1.0 |
| +0x10 | top open bar (A); `extval` = upper bound of the animation range | `maxval`; downfunc saverange |
| +0x14 | value bar; **`extval` is the slider value the renderer reads** | `init` |
| +0x18 | bottom open bar (B) | `minval`; activefunc setcenter, downfunc saverange |
| +0x1c | rate hslider: min 0, max `(maxval−minval)/2` | val 0 (hue ch0: 0.01) |
| +0x20 | bounce toggle (on = reflect, off = wrap) | per-slider argument |

`mkepslider(label, x, y, min, max, init, float *resetptr, int bounce, panel)`. The values
below were decoded from the stacked f64 arguments in `defpanel`:

| id | name | min | max | init | bounce | resetptr |
|---|---|---|---|---|---|---|
| 31 | zoom | −0.5 | 0.5 | 0 | 1 | – |
| 38 | twst | −200 | 200 | 0 | 1 | – |
| 45 | spd | 0 | 1 | 1.0 | 1 | – |
| 52 | n | 1 | 127 (`0x10000ea8`) | 32 | 1 | – |
| 59 | amb (lighting) / rat (no lighting) | 0 / −20 | 1 / 20 | 0 / 1.0 | 1 | – |
| 66 | shn / rng | 0 / 0 | 1 / 100 (4 planes: 0..10) | 1.0 / 1.0 | 1 | – |
| 73 | whl | −60 | 60 | 0 | 1 | &wheel |
| 80 | spn | −20 | 20 | 0 | 1 | &gspin |
| 87 | flp | −20 | 20 | 0 | 1 | &gflip |
| 94 | arm | −3 | 3 | 0 | 1 | – |
| 101 | wrst | −3 | 3 | 0 | 1 | – |
| 108 | sz | 0 | 10 | 1.0 | 1 | – |
| 123 | hue ch0 (`hslider[0]`) | 0 | 2 | 1.0 | 0 | – |
| 130 | light ch0 (`lslider[0]`) | 0 | 1 | 0.5 | 0 | – |

(Hue and light for channels 1–4 use the same parameters, at ids 141/148, 159/166, 177/184
and 195/202. Each colour channel takes 18 ids; channel 4 adds two palettes.)

### 5.3 Id map (from creation order; matches the script comments)

0–11 script panel (`initscriptpanel`: 1 panel + 11 actuators); 12 main panel; 13 mouse;
14 label; 15 exit; 16 reset; **17 auto**; **18 outline (toggle)**; 19 fat lines; 20 fill;
21 ribbons; 22 smooth; 23 smear; 24 fade; 25 back; 26 z-buf; 27 light; 28 stop;
**29 full screen**; 30 position puck; sliders at 31 + 7k (table above); 115 colours panel;
116–121 alpha/back/hue-back dials and their R buttons; 122 `adial[0]`; **123 `hslider[0]`
(127 = its rate)**; **130 `lslider[0]`**; 137 hueindex[0]; 138 dwell[0]; 139 active[0]; …;
214 chanrateslider. With `-t`, the texture panel starts at 315 (`pnl_id = 115 + 200`).
Every button is `ep_mkact`'d even when it is not added to the panel, so the ids do not
depend on the hardware flags.

---

## 6. Implementation notes (Rust port)

```rust
struct EpSlider {            // libpanel multislider + ep user data, flattened
    min: f32, max: f32,      // +0x54 / +0x50
    val: f32, initval: f32,  // +0x44 / +0x4c (val mirrors bar)
    top: f32, bottom: f32,   // A, B (open-bar extvals)
    bar: f32,                // value-bar extval: what drawing reads
    center: f32, range: f32, dir: f32,
    rate: f32, rate_init: f32, rate_max: f32,
    bounce: bool,
}
enum ActKind { Slider(EpSlider), Toggle, WideButton, RateOf(usize /*parent id*/), Dial, Filled, Button, Other }
struct Actuator { id: u16, kind: ActKind, val: f32, initval: f32, min: f32, max: f32,
                  down: Option<Cb>, active: Option<Cb>, up: Option<Cb> }

struct ActRec { enabled: bool, moving: bool, dir: f32, target: f32,
                upper: f32, lower: f32, step: f32, duration: f32, variance: f32 }
// acttable: Vec<Option<(ActuatorId, ActRec)>> indexed by id, len 1024; max_index = 214 (no textures)

enum Op { Frame, AbsFrame, ActLim1, ActLim2, ActSet, ActReset, ActResetAll, ActStop,
          ActStopAll, SeqDo, SeqStop, SeqLoop, SeqKill, SeqKillAll, Duration, Variance, RandDelay }
struct Command { op: Op, iarg: i32, frame: f32, farg: f32 }
struct Sequence { id: i32, active: bool, start: f32, looplen: f32, duration: f32 /*300*/,
                  variance: f32 /*1*/, cmds: Vec<Command>, cur: Option<usize> }
// seq_list: Vec<Sequence> in list order (index 0 = head = most recently created)

struct Rand48 { x: u64 }     // SVID drand48; seed via srand48(seed as u32)
struct Clock { current_frame: f32, auto_released: bool, t: f32, n: i32 }
```

Exact per-frame update order, which is the contract for replay:

1. (UI/event poll; nothing to do in saver mode.)
2. If the stop toggle is on: render and idle until it is off. No state advances.
3. If `aflag || auto_on`:
   a. `taste_queue()`: `current_frame += 1`, then sequences in list order, with due
      commands run in order (randdelay draws here).
   b. `set_act_targets()`: ids ascending in `0..max_index`, each enabled idle record:
      draw u, draw e1, draw e2 if `D1 ≥ 1`.
   c. `animate_acts()`: ids ascending; step, arrival clamp, `setact` (with clamping into
      [B, A] for sliders, callbacks for toggles/auto).
4. `animate_sliders()` in reverse-creation order (no-op when every rate is 0).
5. `t += speed`; `crossed = t ≥ 1`; `n += floor(t)`; `t = fmod(t, 1)`; `gchant` update.
6. Write ring slot `n mod 128` from the current slider and button values; compute `dwheel`,
   `nlimit`, colours, background.
7. If `crossed`: accumulate `gflip` and `gspin` (mod 360) and store them in the ring.
8. `wheel = fmod(wheel − dwheel, 360)`.
9. Clear, fade or smooth as configured; `drawit()`; present.

Keep every quantity that the original stores as `float` in `f32`, and do the
double-precision intermediate steps listed in §2.3 and §3.3 in `f64`. Reproducing a run
also needs the same `srand48` seed; the original seeds with `time(NULL)`, so expose it as
an option and default to the clock. Tick 1 executes seq 0 and tick 2 starts seqs 1–8; a
port must keep this one-tick lag.

## 7. Open questions

1. (Resolved: IRIX `drand48` is the SVID LCG; see `re/fixtures/`.)
2. **UNCERTAIN:** the effective tick rate on real hardware (vsync rate, and whether `drawit`
   ever misses a retrace). Measure fps on an Indy/Indigo2 with the default settings.
3. (Resolved by `ep_S.trace`: with no input, `pnl_dopanel` in `-S` mode leaves every
   animated value unchanged; all quantities match the model.)
4. The behaviour of `seqkill:`/`seqkillall:` is buggy (infinite loop / wrong victim). The
   internal script never uses them. Pick the intended semantics if `-a` scripts must be
   supported.

## 8. Validation against ground-truth IRIX traces

Traces: `re/tracer/ep_long.trace`, 3600 frames, default flags, `time() = 12345`, decoded
with `tools/irix/eptrace_dump.py`. Run the check with:
`python re/tools/check_trace.py re/ghidra/ep-irix53 <decoded.txt> 12345`.
That checker was for the Python model; the Rust port is checked against the same traces
by `cargo test -p ep-core --test traces` (docs/ground-truth.md).
`re/tracer/dump-o.txt` (`ep -o`) matches the parsed tables in §1.4 line for line, including
`seq N at 0.00` and every `cmd` line, and the actuator table confirms ids 17, 18, 29, 31, 38,
52, 73, 80, 87, 94, 101, 108 (labels) and 123/127/130 (unlabelled colour sub-actuators).

**Frame mapping** (found empirically and consistent with the code):
* Trace frame 0 is initialisation, ending with the pre-loop `ep_clear; swapbuffers`.
* Frames 1 and 2 are the two swaps that `fullscreen()` performs, triggered by
  `actset: 29, 1.0` during tick 1.
* So trace frame f is main-loop tick `k = f − 2` for f ≥ 3.
* drawit's first strand segment of tick k shows the actuator values at the end of
  **tick k−1**. drawit draws `twixt/foldtwixt(slot n−1, slot n, t)`, and at the default
  speed `t = 0` after the `fmod`, so it gets slot n−1.

**Compared per frame** for all 3597 loop frames:

| quantity | source in trace | result |
|---|---|---|
| drand48 sequence: count and each value per frame | logged `drand48()` | **0 mismatches** (1160 draws total) |
| arm (94) | `translate(0, arm, 0)` | 0 mismatches (rel. 1e-6) |
| wrist (101) | `translate(wrist, 0, 0)` | 0 mismatches |
| size (108) | `scale(s, s, 1)` | 0 mismatches |
| twist (38) | `rotate(tenths, 'z')` between segments | 0 mismatches (truncated to tenths) |
| wheel accumulator | frame-level `rotate(tenths, 'x')` | 0 mismatches |
| gflip accumulator | `rotate(tenths, 'y')` | 0 mismatches (modulo 360; see below) |
| hue ch0 (123) / light ch0 (130) | first segment `c4f`, inverted with standard HLS | 0 mismatches (tolerance 2e-3) |

**Mismatches found during validation, and how they were resolved:**
1. *Draw count at tick 2: the trace has 28, the seed-0 simulation 29.* Not a model error:
   with seed 12345 the model also gives 28. The `D1 < 1` short path of §3.3 occurs for flip
   (u = 0.00434), which confirms the evaluate-twice duration logic, including the 2-draw
   branch.
2. *Hue colour off by 0.01 in every frame* when `animatesliders` was left out of the model.
   Fixed by modelling the tick-1 self-oscillation step of hue ch0 (§3.6). This is a negative
   control: with the step removed the check fails from tick 4 onward.
3. *Flip at frame 462 is drawn as +1.9° where the model's slot holds −358.08°.* This is
   rendering, not animation: `foldtwixt @0x411050` adds 360 to the older slot when the newer
   slot is more than 180° below it (here the newer slot is −3.08° after the fmod wrap).
   The accumulator value in the model is correct. The checker compares modulo 3600 tenths.
4. *Two frames showed draw "mismatches" with equal counts.* These came from the checker
   rounding to 6 decimals at a rounding boundary. Comparing with a tolerance of 1e-8 against
   the 9-digit trace values gives zero mismatches.

The remaining open questions are §7 items 2–4. The traces were captured at the tracer's own
pace, so they confirm the per-tick logic but not the wall-clock tick rate.

**Other traces** (300 frames each, same seed):
* `ep_S.trace` (`-S`) matches on every quantity with **frame offset 0**: trace frame f is
  tick f. In saver mode the window is opened borderless at full size, so `fullscreen()` adds
  no extra swaps. Run `check_trace.py … 12345 0`.
* `ep_q.trace` (`-q`, quad strands) matches on draws, arm, wrist, size, flip, wheel and
  colour with offset 2. Its "twist" column fails only because the checker's rendering-layout
  heuristic picks a different `rotate(…, 'z')` in quad mode. The animation state is the same
  as in single-strand mode; `-q` changes rendering only.
* `ep200.trace` is identical to `ep200b.trace` and is a prefix-equivalent of the long run.
  The original is therefore deterministic for a given `time()`.
