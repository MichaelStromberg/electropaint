//! The ep main loop: one [`Sim::tick`] is one pass of the original's
//! `main` loop (animation.md §4.1, rendering.md §3).
//!
//! The simulation owns an emulated IrisGL context ([`Gl`]); everything
//! the original draws goes through it, which produces both the renderer's
//! [`Frame`] and (optionally) a call log comparable with IRIX traces.

use std::collections::VecDeque;

use crate::anim::{ActRec, SeqState};
use crate::colour::{hls_to_rgb, rgb_to_irgb};
use crate::frame::{Frame, Op};
use crate::gl::{Arg, Gl, IDENTITY, IrisCall, RgbColorIndex, lm};
use crate::panel::{Accumulator, Actuator, Callback, Controls, ID_LIMIT, Kind, Panel, ids};
use crate::rand48::Rand48;
use crate::script::Script;

/// Ring buffer length for the strand history.
pub(crate) const RING: usize = 128;

/// How a run is set up (the original's command-line flags and hardware).
#[derive(Clone, Debug)]
pub struct Options {
    /// `srand48` seed; the original uses `time(NULL)`.
    pub seed: i64,
    /// `-q`: four mirrored strands.
    pub quad: bool,
    /// `-S`: screensaver mode (no window changes, no colour-map ramp).
    pub saver: bool,
    /// The script to run: the built-in show (default, `-b`), one read
    /// with `-a`, or `None` for `-c` (no script, controls visible).
    pub script: Option<Script>,
    /// `-o` teach mode: releasing a control with "auto" on records its
    /// limits into the random walk.
    pub record: bool,
    /// Screen size as `getgdesc(GD_XPMAX/GD_YPMAX)` reports it; the
    /// full-screen aspect ratio is its ratio.
    pub screen: (i32, i32),
    /// Deliver the window system's REDRAW on the first tick, as on the
    /// traced machine (non-saver mode). Turn off to inject REDRAWs with
    /// [`Sim::redraw`] instead.
    pub startup_redraw: bool,
    /// Record an [`IrisCall`] log.
    pub call_log: bool,
    /// What `color()` does in RGB mode (only the fade uses it).
    pub rgb_color_index: RgbColorIndex,
}

impl Options {
    /// The default show (`ep` with no flags) with the given seed.
    pub fn new(seed: i64) -> Options {
        Options {
            seed,
            quad: false,
            saver: false,
            script: Some(Script::builtin()),
            record: false,
            screen: (1280, 1024),
            startup_redraw: true,
            call_log: false,
            rgb_color_index: RgbColorIndex::PackedLow16,
        }
    }

    /// `-S`: screensaver mode (no startup REDRAW either, as traced).
    pub fn saver(mut self) -> Options {
        self.saver = true;
        self.startup_redraw = false;
        self
    }
}

impl Default for Options {
    /// The default show seeded from the clock, as the original does.
    fn default() -> Options {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Options::new(now)
    }
}

/// Why a `swapbuffers` happened. Ticks normally present once, but the
/// original also swaps at startup, twice when changing between window and
/// full screen, twice per tick in smooth mode, and while stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapCause {
    /// The pre-loop clear (the original's trace frame 0).
    Startup,
    /// `fullscreen()`/`regularscreen()` clearing both buffers.
    ScreenChange,
    /// Smear+smooth: the first of the tick's two draws.
    SmoothFirstPass,
    /// The stop button: a redraw, or an idle swap while stopped.
    Stopped,
    /// The tick's frame.
    Frame,
}

/// User input, applied at the next tick's `pnl_dopanel`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Input {
    Down(u16),
    Up(u16),
    Drag(u16, f32),
    Release(u16),
    Puck(f32, f32),
}

/// The strand history: one entry per slot, `RING` slots.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct History {
    pub x: [f32; RING],
    pub y: [f32; RING],
    pub dzoom: [f32; RING],
    pub arm: [f32; RING],
    pub wrist: [f32; RING],
    pub dtwist: [f32; RING],
    pub flip: [f32; RING],
    pub spin: [f32; RING],
    pub size: [f32; RING],
    pub hue: [f32; RING],
    pub alpha: [f32; RING],
    pub light: [f32; RING],
    pub alphaout: [f32; RING],
    pub shiny: [f32; RING],
    pub ambient: [f32; RING],
    pub outline: [bool; RING],
    pub fatline: [bool; RING],
    pub fill: [bool; RING],
}

impl History {
    fn new() -> Self {
        let z = [0.0; RING];
        History {
            x: z,
            y: z,
            dzoom: z,
            arm: z,
            wrist: z,
            dtwist: z,
            flip: z,
            spin: z,
            size: z,
            hue: z,
            alpha: z,
            light: z,
            alphaout: z,
            shiny: z,
            ambient: z,
            outline: [false; RING],
            fatline: [false; RING],
            fill: [false; RING],
        }
    }
}

/// History slot for strand position `k` (C remainder made positive).
pub(crate) fn slot(k: i32) -> usize {
    k.rem_euclid(RING as i32) as usize
}

/// The whole simulation.
#[derive(Clone, Debug)]
pub struct Sim {
    pub(crate) gl: Gl,
    pub(crate) rng: Rand48,
    pub(crate) controls: Controls,
    pub(crate) recs: Vec<Option<ActRec>>,
    pub(crate) seqs: Vec<SeqState>,
    pub(crate) edit_seq: Option<i32>,
    pub(crate) current_frame: f32,
    pub(crate) auto_released: bool,
    aflag: bool,
    pub(crate) quad: bool,
    saver: bool,
    record: bool,
    screen: (i32, i32),
    // main-loop state
    pub(crate) t: f32,
    pub(crate) n: i32,
    pub(crate) nlimit: i32,
    gchan: usize,
    gchant: f32,
    pub(crate) gcol: f32,
    ghue: f32,
    glight: f32,
    bhue: f32,
    pub(crate) wheel: f32,
    dwheel: f32,
    gflip: f32,
    gspin: f32,
    pub(crate) hist: History,
    bkg: i32,
    pub(crate) bkg_rgba: [f32; 4],
    pub(crate) col_rgba: [f32; 4],
    pub(crate) outline_rgba: [f32; 4],
    fade_phase: usize,
    puck: [f32; 2],
    current_texture: i32,
    /// libpanel's current actuator (`pnl_ca`): the one being held.
    pub(crate) grabbed: Option<u16>,
    stopped_loop: bool,
    panels_visible: bool,
    exited: bool,
    ticks: u64,
    pending_redraw: bool,
    inputs: VecDeque<Input>,
    next_inputs: Vec<Input>,
    swaps: Vec<SwapCause>,
    startup_ops: Vec<Op>,
    /// How the last pass moved, for [`Sim::frame_between`]; `None` unless
    /// it repainted the whole picture.
    between: Option<Between>,
}

/// Values from the start of the last main-loop pass that an in-between
/// picture blends from.
#[derive(Clone, Copy, Debug)]
struct Between {
    speed: f32,
    wheel: f32,
    bkg_rgba: [f32; 4],
}

pub(crate) const NCOLORS: i32 = 128;
pub(crate) const BASECOLOR: i32 = 640;
const HAS_ZBUFFER: bool = true;
const HAS_LIGHTING: bool = true;

/// `fadebackground` dither order: pattern row and bit shift per phase.
const FADE_ROW: [usize; 16] = [0, 2, 0, 2, 1, 3, 1, 3, 1, 3, 1, 3, 0, 2, 0, 2];
const FADE_SHIFT: [u32; 16] = [0, 2, 2, 0, 1, 3, 3, 1, 0, 2, 2, 0, 1, 3, 3, 1];

impl Sim {
    /// Set up as the original does before its main loop: seed the
    /// generator (`srand48(0)` then `srand48(seed)`), configure GL, define
    /// the lights, build the panels and clear the window. The startup
    /// frame is returned at the start of the first [`Sim::tick`].
    pub fn new(opts: Options) -> Sim {
        let controls = Controls::new();
        let recs = (0..ID_LIMIT)
            .map(|i| controls.get(i as u16).map(|_| ActRec::default()))
            .collect();
        let script = opts.script.clone();
        let mut gl = Gl::new(opts.call_log);
        gl.rgb_color_index = opts.rgb_color_index;
        let mut sim = Sim {
            gl,
            rng: Rand48::new(0),
            controls,
            recs,
            seqs: script
                .as_ref()
                .map(SeqState::from_script)
                .unwrap_or_default(),
            edit_seq: script.as_ref().and_then(|s| s.edit_seq),
            current_frame: 0.0,
            auto_released: false,
            aflag: script.is_some(),
            quad: opts.quad,
            saver: opts.saver,
            record: opts.record,
            screen: opts.screen,
            t: 0.0,
            n: 0,
            nlimit: 0,
            gchan: 0,
            gchant: 0.0,
            gcol: 0.0,
            ghue: 0.0,
            glight: 0.0,
            bhue: 0.0,
            wheel: 0.0,
            dwheel: 0.0,
            gflip: 0.0,
            gspin: 0.0,
            hist: History::new(),
            bkg: 0,
            bkg_rgba: [0.0, 0.0, 0.0, 1.0],
            col_rgba: [1.0, 0.0, 0.0, 1.0],
            outline_rgba: [0.0, 0.0, 1.0, 1.0],
            fade_phase: 0,
            puck: [0.0; 2],
            current_texture: 0,
            grabbed: None,
            stopped_loop: false,
            panels_visible: script.is_none(),
            exited: false,
            ticks: 0,
            pending_redraw: opts.startup_redraw,
            inputs: VecDeque::new(),
            next_inputs: Vec::new(),
            swaps: Vec::new(),
            startup_ops: Vec::new(),
            between: None,
        };
        sim.init(opts.seed);
        sim
    }

    fn init(&mut self, seed: i64) {
        // `srand48(0)` then `srand48(time)`: only the second one matters.
        self.gl.log_call("srand48", || vec![Arg::Int(0)]);
        self.rng.srand48(0);
        self.gl
            .log_call("srand48", || vec![Arg::Int(i64::from(seed as i32))]);
        self.rng.srand48(seed);
        let gl = &mut self.gl;
        gl.doublebuffer();
        if HAS_LIGHTING {
            gl.mmode(2);
            deflight(gl);
        }
        gl.rgbmode();
        gl.blendfunction(4, 5);
        gl.subpixel(true);
        gl.gconfig();
        if !self.saver {
            // load_spectrum_map: an HLS hue ramp in the colour map (used by
            // the colours panel).
            for i in 0..NCOLORS {
                let c = hls_to_rgb(f64::from(i as f32 / NCOLORS as f32), 0.5, 1.0).map(rgb_to_irgb);
                gl.mapcolor((BASECOLOR + i) as u16, c[0], c[1], c[2]);
            }
        }
        gl.shademodel(0);
        gl.concave(false);
        gl.glcompat(0, 0);
        gl.perspective(300, 1.0, 0.01, 1.0e6);
        if HAS_LIGHTING {
            gl.loadmatrix(&IDENTITY);
        }
        gl.polarview(10.0, 0, 0, 0);
        gl.lsetdepth(0, 0x7f_ffff);
        gl.c4f(self.bkg_rgba);
        gl.clear();
        self.swap(SwapCause::Startup);
        self.gl.clear();
        self.n = 0;
        self.t = 0.0;
        self.gchan = 0;
        self.gchant = 0.0;
        self.gcol = ((NCOLORS >> 1) + BASECOLOR) as f32;
        self.bhue = 0.5;
        self.ghue = 0.5;
        self.startup_ops = self.gl.take_frame().ops;
    }

    // ------------------------------------------------------------------ API

    /// Run one pass of the main loop and return what it drew. The first
    /// call's frame starts with the startup clear.
    pub fn tick(&mut self) -> Frame {
        self.swaps.clear();
        let mut ops = std::mem::take(&mut self.startup_ops);
        if ops.contains(&Op::Swap) {
            self.swaps.push(SwapCause::Startup);
        }
        if !self.exited {
            self.ticks += 1;
            self.main_loop_pass();
            for i in self.next_inputs.drain(..) {
                self.inputs.push_back(i);
            }
        }
        ops.extend(self.gl.take_frame().ops);
        Frame { ops }
    }

    /// Why each `Swap` in the last tick's frame happened, in order.
    pub fn swap_causes(&self) -> &[SwapCause] {
        &self.swaps
    }

    /// The call log since the last call (empty unless `Options::call_log`).
    pub fn take_calls(&mut self) -> Vec<IrisCall> {
        self.gl.take_calls()
    }

    /// Every actuator, for a UI.
    pub fn actuators(&self) -> impl Iterator<Item = &Actuator> {
        self.controls.iter()
    }

    pub fn actuator(&self, id: u16) -> Option<&Actuator> {
        self.controls.get(id)
    }

    pub fn controls(&self) -> &Controls {
        &self.controls
    }

    /// The value an engine-side reader sees: an ep slider's value bar, or
    /// the actuator's `val`.
    pub fn value(&self, id: u16) -> Option<f32> {
        let a = self.controls.get(id)?;
        Some(match a.kind {
            Kind::Slider => self.controls.slider_value(id),
            Kind::SliderUpper | Kind::SliderLower | Kind::SliderValue => a.extval,
            _ => a.val,
        })
    }

    /// Whether a toggle or button reads as on.
    pub fn is_on(&self, id: u16) -> bool {
        self.controls.get(id).is_some_and(Actuator::on)
    }

    /// Drag a control to `v` (applied at the next tick): ep sliders move
    /// their value bar (shoving the bounds along, as libpanel's push mode
    /// does), bound bars move the animation window, dials and plain
    /// sliders take the value; the control counts as held for that tick.
    pub fn set_value(&mut self, id: u16, v: f32) {
        self.inputs.push_back(Input::Drag(id, v));
        self.next_inputs.push(Input::Release(id));
    }

    /// Click a button or toggle (pressed at the next tick, released at the
    /// one after): toggles flip, momentary buttons fire.
    pub fn press(&mut self, id: u16) {
        self.inputs.push_back(Input::Down(id));
        self.next_inputs.push(Input::Up(id));
    }

    /// Press a key equivalent (`'q'` smear, `'\x1b'` exit, …). Returns
    /// whether the key is bound.
    pub fn key(&mut self, key: char) -> bool {
        match self.controls.key_target(key) {
            Some(id) => {
                self.press(id);
                true
            }
            None => false,
        }
    }

    /// Move the position puck (strand offset).
    pub fn set_puck(&mut self, x: f32, y: f32) {
        self.inputs.push_back(Input::Puck(x, y));
    }

    /// Deliver a window-system REDRAW (window mapped or resized) at the
    /// next tick: the projection is reloaded and the window cleared.
    pub fn redraw(&mut self) {
        self.pending_redraw = true;
    }

    /// Whether the panels would be showing (they hide in full screen).
    pub fn panels_visible(&self) -> bool {
        self.panels_visible
    }

    /// Whether full-screen mode is on.
    pub fn fullscreen(&self) -> bool {
        self.is_on(ids::FULLSCREEN)
    }

    /// Whether "exit" was triggered.
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// The picture a fraction `alpha` (0..1) of the way through the last
    /// tick, for displays that refresh faster than the original's 60 Hz.
    /// Returns ops to draw into the back buffer (no swap), or `None` when
    /// the last tick didn't repaint the whole picture (smear and fade
    /// accumulate trails, which an extra picture would add to).
    ///
    /// The picture is drawn by ep's own strand code from a copy of the
    /// simulation, with the strand phase set `(1 - alpha) * speed` behind
    /// where the tick left it. ep places every slot by blending neighbouring
    /// history entries by that phase, so this is the in-between position the
    /// original's own mechanism gives: slots stay rigid and rotations
    /// interpolate as angles. The wheel angle and background colour, which
    /// are not kept in the history, are blended from the start of the tick.
    pub fn frame_between(&self, alpha: f32) -> Option<Frame> {
        let b = self.between?;
        if self.exited {
            return None;
        }
        let mut s = self.clone();
        s.gl.take_frame();
        let back = f64::from((1.0 - alpha) * b.speed);
        let phase = f64::from(self.n) + f64::from(self.t) - back;
        s.n = phase.floor() as i32;
        s.t = (phase - phase.floor()) as f32;
        let mut turn = self.wheel - b.wheel;
        if turn > 180.0 {
            turn -= 360.0;
        } else if turn < -180.0 {
            turn += 360.0;
        }
        s.wheel = b.wheel + turn * alpha;
        for i in 0..4 {
            s.bkg_rgba[i] = b.bkg_rgba[i] + (self.bkg_rgba[i] - b.bkg_rgba[i]) * alpha;
        }
        s.gl.c4f(s.bkg_rgba);
        s.gl.clear();
        s.drawit();
        Some(s.gl.take_frame())
    }

    /// Strand slot counter `n` and how many slots are drawn.
    pub fn strand(&self) -> (i32, i32) {
        (self.n, self.nlimit)
    }

    // ------------------------------------------------------------------ loop

    fn swap(&mut self, cause: SwapCause) {
        self.gl.swapbuffers();
        self.swaps.push(cause);
    }

    fn slider(&self, id: u16) -> f32 {
        self.controls.slider_value(id)
    }

    fn main_loop_pass(&mut self) {
        self.between = None;
        let (wheel0, bkg0) = (self.wheel, self.bkg_rgba);
        // 1. pnl_dopanel
        self.dopanel();
        // 2. stop: redraw once, then idle-swap until released.
        if self.stopped_loop {
            self.swap(SwapCause::Stopped);
            if self.controls.act(ids::STOP).on() {
                return;
            }
            self.stopped_loop = false;
        } else if self.controls.act(ids::STOP).on() {
            if !self.controls.act(ids::SMEAR).not_off() {
                self.gl.c4f(self.bkg_rgba);
                self.gl.clear();
            }
            self.drawit();
            self.swap(SwapCause::Stopped);
            self.stopped_loop = true;
            return;
        }
        // 3. animation
        if self.aflag || self.controls.act(ids::AUTO).on() {
            self.taste_queue();
            self.set_act_targets();
            self.animate_acts();
        }
        // 4. self-oscillation
        self.animate_sliders();
        // 5. time step
        let speed = self.slider(ids::SPEED);
        self.t += speed;
        let p0 = f64::from(self.t);
        self.n = (f64::from(self.n) + p0.floor()) as i32;
        self.t = (p0 % 1.0) as f32;
        let chanrate = self.controls.act(ids::CHAN_RATE).val;
        self.gchant = (f64::from(self.gchant + chanrate * speed) % 1.0) as f32;
        // 6. record the head slot
        let s = slot(self.n);
        self.hist.x[s] = self.puck[0];
        self.hist.y[s] = self.puck[1];
        self.hist.dzoom[s] = self.slider(ids::ZOOM);
        self.hist.arm[s] = self.slider(ids::ARM);
        self.hist.wrist[s] = self.slider(ids::WRIST);
        self.dwheel = speed * self.slider(ids::WHEEL);
        self.hist.dtwist[s] = self.slider(ids::TWIST);
        self.nlimit = self.slider(ids::N) as i32;
        self.hist.outline[s] = self.controls.act(ids::OUTLINE).on();
        self.hist.fatline[s] = self.controls.act(ids::FATLINE).on();
        self.hist.fill[s] = self.controls.act(ids::FILL).on();
        self.hist.size[s] = self.slider(ids::SIZE);
        // RGB: alpha and colour come from the channel chosen last tick.
        self.hist.alpha[s] = self.controls.act(ids::alpha_dial(self.gchan)).val;
        self.hist.alphaout[s] = self.controls.act(ids::ALPHA_OUT).val;
        self.hist.shiny[s] = self.slider(ids::SHINY);
        self.hist.ambient[s] = self.slider(ids::AMBIENT);
        self.ghue = self.slider(ids::hue_slider(self.gchan));
        self.glight = self.slider(ids::light_slider(self.gchan));
        let rate = self.controls.act(ids::HUE_BACK_RATE).val;
        self.bhue = (f64::from(self.bhue) + f64::from(speed * rate) * 0.05) as f32;
        // 7. wrap the hues into [0,1)
        self.ghue = wrap_unit(self.ghue);
        self.bhue = wrap_unit(self.bhue);
        // 8. channel selection and the channel swatches
        self.hist.hue[s] = self.ghue;
        self.hist.light[s] = self.glight;
        self.choose_channel();
        for i in 0..5 {
            let h = (f64::from(self.slider(ids::hue_slider(i))) % 1.0) as f32;
            let c = hls_to_rgb(
                f64::from(h),
                f64::from(self.slider(ids::light_slider(i))),
                1.0,
            )
            .map(rgb_to_irgb);
            self.gl.mapcolor(0x300 + i as u16, c[0], c[1], c[2]);
        }
        // 9. background
        if self.controls.act(ids::BACKGROUND).on() {
            self.bkg = self.gcol as i32 + (NCOLORS >> 1);
            if BASECOLOR + NCOLORS <= self.bkg {
                self.bkg -= NCOLORS;
            }
            let c = hls_to_rgb(f64::from(self.bhue), 0.5, 1.0);
            self.bkg_rgba[..3].copy_from_slice(&c);
        } else {
            self.bkg = 0;
            self.bkg_rgba[..3].copy_from_slice(&[0.0; 3]);
        }
        self.bkg_rgba[3] = self.controls.act(ids::ALPHA_BACK).val;
        // 10. flip and spin accumulate once per new slot
        if 1.0 <= p0 {
            let dflip = self.slider(ids::FLIP);
            let dspin = self.slider(ids::SPIN);
            self.gflip = (f64::from(self.gflip + dflip) % 360.0) as f32;
            self.hist.flip[s] = self.gflip;
            self.gspin = (f64::from(self.gspin + dspin) % 360.0) as f32;
            self.hist.spin[s] = self.gspin;
        }
        // 11. window events
        if std::mem::take(&mut self.pending_redraw) {
            self.reshape();
        }
        // 12. wheel
        self.wheel = (f64::from(self.wheel - self.dwheel) % 360.0) as f32;
        // 13. buffers
        if !self.controls.act(ids::SMEAR).not_off() {
            self.gl.c4f(self.bkg_rgba);
            self.gl.clear();
            self.between = Some(Between {
                speed,
                wheel: wheel0,
                bkg_rgba: bkg0,
            });
        } else {
            if self.controls.act(ids::FADE).on() {
                self.fade_background(1);
            }
            if self.controls.act(ids::SMOOTH).on() {
                self.drawit();
                self.swap(SwapCause::SmoothFirstPass);
                if self.controls.act(ids::FADE).on() {
                    self.fade_background(0);
                }
            }
        }
        // 14. draw and present
        self.drawit();
        self.swap(SwapCause::Frame);
    }

    /// Channel selection: a held active button wins; otherwise the first
    /// channel whose cumulative dwell exceeds the channel clock, and that
    /// channel's active button is lit.
    fn choose_channel(&mut self) {
        if let Some(i) = (0..5).find(|&i| self.grabbed == Some(ids::active_button(i))) {
            self.gchan = i;
            return;
        }
        let mut acc = 0.0f32;
        self.gchan = 0;
        for i in 0..5 {
            acc += self.controls.act(ids::dwell(i)).val;
            if self.gchant < acc {
                self.gchan = i;
                break;
            }
        }
        let b = self.controls.act(ids::active_button(self.gchan));
        if !b.not_off() {
            for i in 0..5 {
                self.controls.act_mut(ids::active_button(i)).val = 0.0;
            }
            self.controls.act_mut(ids::active_button(self.gchan)).val = 1.0;
        }
    }

    /// REDRAW: reload the projection and clear.
    fn reshape(&mut self) {
        self.gl.reshapeviewport();
        let aspect = if self.controls.act(ids::FULLSCREEN).on() {
            self.screen_aspect()
        } else {
            1.0
        };
        self.reset_view(aspect);
        self.gl.c4f(self.bkg_rgba);
        self.gl.clear();
    }

    fn screen_aspect(&self) -> f32 {
        self.screen.0 as f32 / self.screen.1 as f32
    }

    fn reset_view(&mut self, aspect: f32) {
        self.gl.perspective(300, aspect, 0.01, 10000.0);
        if HAS_LIGHTING {
            self.gl.loadmatrix(&IDENTITY);
        }
        self.gl.polarview(10.0, 0, 0, 0);
        self.gl.lsetdepth(0, 0x7f_ffff);
    }

    /// `fullscreen()`: refused while stopped; otherwise hide the panels,
    /// take the whole screen and clear both buffers.
    fn go_fullscreen(&mut self) {
        self.controls.act_mut(ids::FULLSCREEN).val = 1.0;
        if self.controls.act(ids::STOP).on() {
            self.controls.act_mut(ids::FULLSCREEN).val = 0.0;
            return;
        }
        self.panels_visible = false;
        self.gl.reshapeviewport();
        let aspect = self.screen_aspect();
        self.reset_view(aspect);
        self.gl.c4f(self.bkg_rgba);
        self.gl.clear();
        self.swap(SwapCause::ScreenChange);
        self.gl.clear();
        self.swap(SwapCause::ScreenChange);
    }

    /// `regularscreen()`: back to the window, panels shown.
    fn go_regularscreen(&mut self) {
        self.controls.act_mut(ids::FULLSCREEN).val = 0.0;
        self.panels_visible = true;
        self.gl.reshapeviewport();
        self.reset_view(1.0);
        self.gl.c4f(self.bkg_rgba);
        self.gl.clear();
        self.swap(SwapCause::ScreenChange);
        self.gl.clear();
        self.swap(SwapCause::ScreenChange);
    }

    /// `fadebackground(idx, step)`: overwrite one pixel of every 4x4 block
    /// with colour index `idx` through a stipple; 16 steps cover them all.
    fn fade_background(&mut self, step: usize) {
        self.fade_phase += step;
        while self.fade_phase > 15 {
            self.fade_phase -= 16;
        }
        let mut pat = [0u16; 16];
        let r = FADE_ROW[self.fade_phase];
        let w = (0x1111u32 << FADE_SHIFT[self.fade_phase]) as u16;
        for k in [r, r + 4, r + 8, r + 12] {
            pat[k] = w;
        }
        self.gl.defpattern(2, pat);
        self.gl.color(self.bkg as u16);
        self.gl.setpattern(2);
        self.gl.clear();
        self.gl.setpattern(0);
    }

    // ------------------------------------------------------------------ panel

    /// `pnl_dopanel`: apply queued input.
    fn dopanel(&mut self) {
        while let Some(i) = self.inputs.pop_front() {
            match i {
                Input::Down(id) => self.act_down(id),
                Input::Up(id) => self.act_up(id),
                Input::Drag(id, v) => self.act_drag(id, v),
                Input::Release(id) => {
                    if self.controls.get(id).is_some() {
                        self.panel_callback(id, false);
                        self.callback(self.controls.act(id).up, id);
                    }
                    self.grabbed = None;
                }
                Input::Puck(x, y) => self.puck = [x, y],
            }
        }
    }

    fn act_down(&mut self, id: u16) {
        let Some(a) = self.controls.get_mut(id) else {
            return;
        };
        match a.kind {
            Kind::Toggle | Kind::SliderBounce | Kind::WideToggle => {
                a.val = if a.val == a.min { a.max } else { a.min };
            }
            Kind::Button | Kind::WideButton | Kind::SliderReset | Kind::DialReset | Kind::Mouse => {
                a.val = a.max
            }
            Kind::Radio => a.val = a.max,
            _ => {}
        }
        let (down, active) = (a.down, a.active);
        self.grabbed = Some(id);
        self.callback(down, id);
        self.callback(active, id);
        self.panel_callback(id, true);
    }

    fn act_up(&mut self, id: u16) {
        let Some(a) = self.controls.get_mut(id) else {
            return;
        };
        if matches!(
            a.kind,
            Kind::Button | Kind::WideButton | Kind::SliderReset | Kind::DialReset | Kind::Mouse
        ) {
            a.val = a.min;
        }
        let up = a.up;
        self.callback(up, id);
        self.panel_callback(id, false);
        self.grabbed = None;
    }

    /// A drag on a slider-like control.
    fn act_drag(&mut self, id: u16, v: f32) {
        let Some(a) = self.controls.get(id).cloned() else {
            return;
        };
        let clamp = |v: f32| v.clamp(a.min.min(a.max), a.max.max(a.min));
        self.grabbed = Some(id);
        match (a.kind, a.slider) {
            (Kind::Slider | Kind::SliderValue, Some(s)) => {
                let sl = self.controls.sliders[s].clone();
                let main = self.controls.act(sl.id).clone();
                let bar = v.clamp(main.min, main.max);
                let up = self.controls.act_mut(sl.upper());
                up.extval = up.extval.max(bar);
                let lo = self.controls.act_mut(sl.lower());
                lo.extval = lo.extval.min(bar);
                self.controls.act_mut(sl.value()).extval = bar;
                self.controls.act_mut(sl.id).val = bar;
                self.grabbed = Some(sl.value());
            }
            (Kind::SliderUpper | Kind::SliderLower, Some(s)) => {
                self.controls.save_range(s);
                let sl = self.controls.sliders[s].clone();
                let main = self.controls.act(sl.id).clone();
                let x = v.clamp(main.min, main.max);
                let (up, lo, val) = (sl.upper(), sl.lower(), sl.value());
                if a.kind == Kind::SliderUpper {
                    self.controls.act_mut(up).extval = x;
                    let vv = self.controls.act(val).extval.min(x);
                    self.controls.act_mut(val).extval = vv;
                    let b = self.controls.act(lo).extval.min(vv);
                    self.controls.act_mut(lo).extval = b;
                } else {
                    self.controls.act_mut(lo).extval = x;
                    let vv = self.controls.act(val).extval.max(x);
                    self.controls.act_mut(val).extval = vv;
                    let t = self.controls.act(up).extval.max(vv);
                    self.controls.act_mut(up).extval = t;
                    self.controls.set_center(s);
                }
                self.controls.act_mut(sl.id).val = x;
            }
            (
                Kind::SliderRate | Kind::HSlider | Kind::FilledSlider | Kind::Dial | Kind::Palette,
                _,
            ) => {
                self.controls.act_mut(id).val = clamp(v);
                self.callback(a.down, id);
                self.callback(a.active, id);
            }
            _ => {
                self.setact(id, v);
                return;
            }
        }
        self.panel_callback(id, true);
    }

    /// The panels' own callbacks: the main panel records actuator limits
    /// (`recordact`); the colours panel also applies dwell and active
    /// button constraints first.
    fn panel_callback(&mut self, id: u16, down: bool) {
        let Some(panel) = self.controls.get(id).and_then(|a| a.panel) else {
            return;
        };
        match panel {
            Panel::Main => self.record_act(id, down),
            Panel::Colors => {
                if down {
                    self.controls.choose_updates(id);
                }
                self.record_act(id, down);
            }
            Panel::Texture => {}
        }
    }

    /// `recordact`: with "auto" on, pressing a control sets its random
    /// walk's lower limit to the current value; with `-o`, releasing it
    /// sets the upper one and enables the walk.
    fn record_act(&mut self, id: u16, down: bool) {
        if !self.controls.act(ids::AUTO).on() || self.recs[usize::from(id)].is_none() {
            return;
        }
        let v = self.controls.act(id).val;
        if down {
            self.rec_mut(id).lower = v;
        } else if self.record {
            // The duration comes from the sequence the parser last edited;
            // the variance is forced to 1.
            let duration = self
                .edit_seq
                .and_then(|e| self.seqs.iter().find(|s| s.id == e))
                .map_or(300.0, |s| s.duration);
            let r = self.rec_mut(id);
            if r.lower < v {
                r.upper = v;
            } else {
                r.upper = r.lower;
                r.lower = v;
            }
            r.duration = duration;
            r.variance = 1.0;
            r.enabled = true;
        }
    }

    /// `setact`: set a value the way scripts (and the random walks) do,
    /// then call the actuator's down, active and up callbacks.
    pub(crate) fn setact(&mut self, id: u16, v: f32) {
        let a = self.controls.act(id);
        if !a.in_acttable {
            // An "R" button: acttable holds an orphan copy with no
            // callbacks, so nothing visible happens.
            self.controls.act_mut(id).orphan_val = v;
            return;
        }
        let (kind, slider, cbs) = (a.kind, a.slider, [a.down, a.active, a.up]);
        match (kind, slider) {
            (Kind::Slider, Some(s)) => self.controls.set_slider_bar(s, v),
            (Kind::FilledSlider, _) => {
                self.controls.act_mut(id).val = v;
                self.controls.choose_updates(id);
            }
            _ => self.controls.act_mut(id).val = v,
        }
        for cb in cbs {
            self.callback(cb, id);
        }
    }

    fn callback(&mut self, cb: Option<Callback>, id: u16) {
        let Some(cb) = cb else { return };
        let on = |s: &Sim, id: u16| s.controls.act(id).on();
        match cb {
            Callback::SetFullscreen => {
                if self.saver {
                    return;
                }
                let full = on(self, ids::FULLSCREEN);
                // From the full screen button: follow it; from anything
                // else (the mouse): toggle.
                if full == (id == ids::FULLSCREEN) {
                    self.go_fullscreen();
                } else {
                    self.go_regularscreen();
                }
            }
            Callback::CleanExit => self.exited = true,
            Callback::Reset => self.reset(),
            Callback::SetAuto => {
                if !self.controls.act(id).not_off() {
                    self.stop_anim();
                }
            }
            Callback::SetSmooth => {
                if on(self, ids::SMEAR) {
                    let smooth = on(self, id);
                    self.gl.frontbuffer(!smooth);
                    self.gl.backbuffer(true);
                }
            }
            Callback::SetSmear => {
                if on(self, id) {
                    if !self.controls.act(ids::SMOOTH).not_off() {
                        if !self.controls.act(ids::STOP).not_off() {
                            self.gl.c4f(self.bkg_rgba);
                            self.gl.clear();
                            self.drawit();
                        }
                        self.gl.frontbuffer(true);
                        self.gl.backbuffer(true);
                    }
                } else {
                    self.gl.frontbuffer(false);
                    self.gl.backbuffer(true);
                }
            }
            Callback::SetZbuf => {
                if !self.controls.act(id).not_off() {
                    self.gl.zbuffer(false);
                } else {
                    self.gl.zbuffer(true);
                    if HAS_ZBUFFER {
                        self.gl.zclear();
                    }
                }
            }
            Callback::SaveRange | Callback::SetCenter | Callback::SetRange => {
                if let Some(s) = self.controls.get(id).and_then(|a| a.slider) {
                    match cb {
                        Callback::SaveRange => self.controls.save_range(s),
                        Callback::SetCenter => self.controls.set_center(s),
                        _ => self.controls.set_range(s),
                    }
                }
            }
            Callback::ResetSlider => {
                if let Some(s) = self.controls.get(id).and_then(|a| a.slider) {
                    let init = self.controls.act(self.controls.sliders[s].id).initval;
                    match self.controls.reset_slider(s) {
                        Some(Accumulator::Wheel) => self.wheel = init,
                        Some(Accumulator::Spin) => self.gspin = init,
                        Some(Accumulator::Flip) => self.gflip = init,
                        Some(Accumulator::Texture) | None => {}
                    }
                }
            }
            Callback::ResetDial => self.controls.reset_dial(id - 1),
            Callback::ToggleChanMode => self.controls.toggle_chan_mode(id),
            Callback::SetTextureIndex => self.current_texture = 0,
        }
    }

    /// The "reset" button (`reset()`): every "R" pressed twice, the puck
    /// centred, and the buttons, dials and channel controls back to their
    /// initial values (with the buffer callbacks re-run). Random walks
    /// keep going.
    pub(crate) fn reset(&mut self) {
        for id in self.controls.reset_list.clone() {
            let cb = self.controls.act(id).down;
            self.callback(cb, id);
            self.callback(cb, id);
        }
        self.dwheel = 0.0;
        self.puck = [0.0; 2];
        let restore = |s: &mut Sim, id: u16| {
            let a = s.controls.act_mut(id);
            if a.panel.is_some() {
                a.val = a.initval;
            }
        };
        restore(self, ids::SMEAR);
        self.callback(Some(Callback::SetSmear), ids::SMEAR);
        restore(self, ids::ZBUF);
        self.callback(Some(Callback::SetZbuf), ids::ZBUF);
        restore(self, ids::SMOOTH);
        self.callback(Some(Callback::SetSmooth), ids::SMOOTH);
        for id in [
            ids::OUTLINE,
            ids::FILL,
            ids::FATLINE,
            ids::FADE,
            ids::RIBBON,
            ids::BACKGROUND,
            ids::ALPHA_OUT,
            ids::ALPHA_BACK,
            ids::HUE_BACK_RATE,
            ids::LIGHT,
        ] {
            restore(self, id);
        }
        for i in 0..5 {
            restore(self, ids::alpha_dial(i));
            restore(self, ids::dwell(i));
        }
        for id in [ids::CHAN_RATE, ids::HUE_PALETTE, ids::LIGHT_PALETTE] {
            restore(self, id);
        }
        self.controls.toggle_chan_mode(ids::LIGHT_PALETTE);
    }

    /// `uselight`: bind lights 5 and 6, the material and the lighting model.
    pub(crate) fn uselight(&mut self) {
        if HAS_LIGHTING {
            self.gl.lmbind(lm::LIGHT0 + 5, 5);
            self.gl.lmbind(lm::MATERIAL, 1);
            self.gl.lmbind(lm::LIGHT0 + 6, 6);
            self.gl.lmbind(lm::LMODEL, 1);
        }
    }

    /// `ep_lmcolor`: redefine material 1 from a colour and bind it.
    /// `amb` multiplies the colour for the ambient term; `None` stands for
    /// the uninitialised value the ribbon code passes (logged as NaN,
    /// treated as 0).
    pub(crate) fn lmcolor(&mut self, rgba: [f32; 4], amb: Option<f32>, shiny: f32) {
        if !HAS_LIGHTING {
            return;
        }
        let a = |c: f32| amb.map_or(f32::NAN, |m| c * m);
        let shininess = (f64::from(shiny) * 128.0).floor() as f32;
        let props = [
            2.0,
            a(rgba[0]),
            a(rgba[1]),
            a(rgba[2]),
            3.0,
            rgba[0],
            rgba[1],
            rgba[2],
            4.0,
            1.0,
            1.0,
            1.0,
            5.0,
            shininess,
            7.0,
            rgba[3],
            0.0,
        ];
        self.gl.lmdef(lm::DEFMATERIAL, 1, &props);
        self.gl.lmbind(lm::MATERIAL, 1);
    }
}

/// Wrap into [0,1) by repeated single-precision ±1.
fn wrap_unit(mut h: f32) -> f32 {
    while h < 0.0 {
        h += 1.0;
    }
    while 1.0 <= h {
        h -= 1.0;
    }
    h
}

/// `deflight`: material 1, lights 1–7 and lighting model 1.
fn deflight(gl: &mut Gl) {
    gl.lmdef(
        lm::DEFMATERIAL,
        1,
        &[
            2.0, 0.2, 0.2, 0.2, 3.0, 0.8, 0.8, 0.8, 4.0, 1.0, 1.0, 1.0, 5.0, 80.0, 0.0,
        ],
    );
    let positions: [[f32; 3]; 7] = [
        [1.0, 0.0, 0.5],
        [-1.0, 0.0, -0.5],
        [1.0, 0.0, -0.5],
        [-1.0, 0.0, 0.5],
        [0.0, 1.0, 0.5],
        [0.0, -1.0, -0.5],
        [0.0, 1.0, -0.5],
    ];
    for (i, p) in positions.iter().enumerate() {
        gl.lmdef(
            lm::DEFLIGHT,
            i as i16 + 1,
            &[101.0, 1.0, 1.0, 1.0, 102.0, p[0], p[1], p[2], 0.0, 0.0],
        );
    }
    gl.lmdef(
        lm::DEFLMODEL,
        1,
        &[2.0, 1.0, 1.0, 1.0, 201.0, 0.0, 202.0, 1.0, 1.0, 0.0],
    );
}
