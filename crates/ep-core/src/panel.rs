//! The control panel: every actuator ep creates, with the metadata a UI
//! needs (controls.md §3–§5) and the pure value logic of the composite
//! "ep sliders" (animation.md §3.5–§3.7).
//!
//! Ids are libpanel's: one global counter shared by panels and actuators,
//! so they are fixed for this build (texture panel without `.rgb` files).
//! Scripts address actuators by these ids.
//!
//! Callbacks that touch the simulation or GL state (smear, z-buffer,
//! full screen, reset, …) are only *named* here ([`Callback`]); the
//! simulation dispatches them.

mod build;
pub mod ids;

use build::build_panels;

/// Size of the id space allocated in this build (ids above 345 are unused).
pub(crate) const ID_LIMIT: usize = 346;
/// `acttablemaxindex`: the largest id passed through `ep_mkact`. The
/// per-frame animation loops run `i < MAX_INDEX`, so id 345 itself is never
/// animated (an off-by-one in the original).
pub(crate) const MAX_INDEX: usize = 345;

/// Which panel window an actuator lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    /// "electro-paint" (id 12).
    Main,
    /// "colors" (id 115).
    Colors,
    /// "texture" (id 315).
    Texture,
}

/// What kind of control an actuator is (libpanel type, ep's role).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Invisible pseudo-actuator: a click in the graphics window.
    Mouse,
    /// Static text.
    Label,
    /// Momentary button: max while held, min after release.
    Button,
    /// Momentary wide button with the label inside.
    WideButton,
    /// Wide button turned into a toggle ("auto").
    WideToggle,
    /// On/off toggle (flips on press).
    Toggle,
    /// Radio button (texture choice).
    Radio,
    /// 2-D puck (strand offset).
    Puck,
    /// Main actuator of an ep slider (multislider); see [`EpSlider`].
    Slider,
    /// Upper bound bar of an ep slider.
    SliderUpper,
    /// Lower bound bar of an ep slider.
    SliderLower,
    /// Value bar of an ep slider (its `extval` is the slider value).
    SliderValue,
    /// Self-oscillation rate of an ep slider.
    SliderRate,
    /// Bounce (on) / wrap (off) toggle of an ep slider; its range is
    /// inverted (min 1, max 0).
    SliderBounce,
    /// The "R" button of an ep slider.
    SliderReset,
    /// Rotary dial.
    Dial,
    /// The "R" button of a dial.
    DialReset,
    /// Horizontal slider.
    HSlider,
    /// Filled vertical slider (channel dwell).
    FilledSlider,
    /// Colour swatch or palette strip.
    Palette,
    /// The hue palette frame (a frame actuator drawn as a double palette).
    PaletteFrame,
}

/// Callbacks the original attaches to actuators. libpanel calls `down` on
/// the press, `active` while held and `up` on release; a script `actset`
/// calls all three in that order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Callback {
    /// Follow (from the full screen button) or toggle (from anything else)
    /// the full-screen state. Does nothing in screensaver mode.
    SetFullscreen,
    /// Quit.
    CleanExit,
    /// The panel's "reset" button.
    Reset,
    /// Auto toggle: turning it off stops every random walk.
    SetAuto,
    /// Smooth toggle: buffer selection while smearing.
    SetSmooth,
    /// Smear toggle: stop clearing; draw into both buffers.
    SetSmear,
    /// Z-buffer toggle.
    SetZbuf,
    /// Remember a slider's window centre and width.
    SaveRange,
    /// Move the slider's window with its lower bound.
    SetCenter,
    /// Re-clamp after a rate change.
    SetRange,
    /// Slider "R": restore bounds and rate, then value, then accumulator.
    ResetSlider,
    /// Dial "R": restore the dial value.
    ResetDial,
    /// Switch the colour panel between hue and lightness sliders.
    ToggleChanMode,
    /// Choose the texture (radio buttons).
    SetTextureIndex,
}

/// Accumulators that a slider's "R" button resets on its second stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accumulator {
    Wheel,
    Spin,
    Flip,
    /// The four texture sliders point at ring-buffer *pointers* in the
    /// original, so their second stage would corrupt memory there; the
    /// port does nothing for them.
    Texture,
}

/// One actuator: libpanel state plus UI metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Actuator {
    pub id: u16,
    pub label: Option<&'static str>,
    pub kind: Kind,
    /// The panel it was added to; `None` for actuators ep creates but never
    /// adds (fade): those have no key equivalent and are not drawn.
    pub panel: Option<Panel>,
    /// Position and size in panel units (1 unit = 40 px; y points up).
    pub pos: [f32; 2],
    pub size: [f32; 2],
    /// Key equivalent (press and release act on the actuator).
    pub key: Option<char>,
    pub min: f32,
    pub max: f32,
    /// `val` (+0x44): what the engine reads for buttons, dials and plain
    /// sliders.
    pub val: f32,
    /// `initval`, used by reset.
    pub initval: f32,
    /// `extval` (+0x48): the position of a multislider sub-bar.
    pub extval: f32,
    /// The ep slider this actuator belongs to (index into
    /// [`Controls::sliders`]).
    pub slider: Option<usize>,
    /// The owning channel for colour-panel actuators.
    pub channel: Option<usize>,
    pub down: Option<Callback>,
    pub active: Option<Callback>,
    pub up: Option<Callback>,
    pub visible: bool,
    /// Whether `acttable` holds this actuator (and scripts can reach it).
    /// For "R" buttons it holds a never-added orphan copy instead, so
    /// script commands aimed at them only change `orphan_val`.
    pub in_acttable: bool,
    pub orphan_val: f32,
}

impl Actuator {
    fn new(id: u16, kind: Kind, panel: Option<Panel>) -> Self {
        Actuator {
            id,
            label: None,
            kind,
            panel,
            pos: [0.0; 2],
            size: [0.0; 2],
            key: None,
            min: 0.0,
            max: 1.0,
            val: 0.0,
            initval: 0.0,
            extval: 0.0,
            slider: None,
            channel: None,
            down: None,
            active: None,
            up: None,
            visible: true,
            in_acttable: true,
            orphan_val: 0.0,
        }
    }

    /// The toggle test used everywhere in ep: `val > (min+max)/2`, computed
    /// in double precision.
    pub fn on(&self) -> bool {
        mid(self) < f64::from(self.val)
    }

    /// The other form some call sites use: "off" is `val < (min+max)/2`, so
    /// a value exactly at the midpoint counts as on.
    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "exactly the original's test, NaN included"
    )]
    pub fn not_off(&self) -> bool {
        !(f64::from(self.val) < mid(self))
    }

    /// The range as `(low, high)`, whichever way round `min` and `max` are.
    pub fn span(&self) -> (f32, f32) {
        (self.min.min(self.max), self.min.max(self.max))
    }
}

fn mid(a: &Actuator) -> f64 {
    f64::from(a.max) * 0.5 + f64::from(a.min) * 0.5
}

/// A composite ep slider (`mkepslider`): seven actuators with consecutive
/// ids, plus ep's user data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EpSlider {
    pub label: Option<&'static str>,
    /// Id of the main multislider; the others are at fixed offsets
    /// (see [`ids::SLIDER_UPPER`] …).
    pub id: u16,
    /// Saved centre and width of the animation window (`saverange`).
    pub center: f32,
    pub range: f32,
    /// Direction of self-oscillation in bounce mode (±1).
    pub dir: f32,
    /// What the "R" button's second stage resets.
    pub accumulator: Option<Accumulator>,
}

impl EpSlider {
    pub fn upper(&self) -> u16 {
        self.id + ids::SLIDER_UPPER
    }
    pub fn lower(&self) -> u16 {
        self.id + ids::SLIDER_LOWER
    }
    pub fn value(&self) -> u16 {
        self.id + ids::SLIDER_VALUE
    }
    pub fn rate(&self) -> u16 {
        self.id + ids::SLIDER_RATE
    }
    pub fn bounce(&self) -> u16 {
        self.id + ids::SLIDER_BOUNCE
    }
    pub fn reset(&self) -> u16 {
        self.id + ids::SLIDER_RESET
    }
}

/// Clamp `v` into the span between `hi` and `lo` (either order), exactly as
/// the original's inline comparisons do (a NaN lands on `lo`, or on `hi`
/// when the span is reversed).
fn clamp_span(v: f32, hi: f32, lo: f32) -> f32 {
    if lo < hi {
        if lo <= v {
            if v <= hi { v } else { hi }
        } else {
            lo
        }
    } else if hi <= v {
        if v <= lo { v } else { lo }
    } else {
        hi
    }
}

/// Every actuator, indexed by id, plus the ep sliders.
#[derive(Clone, Debug, PartialEq)]
pub struct Controls {
    acts: Vec<Option<Actuator>>,
    pub sliders: Vec<EpSlider>,
    /// `rblist`: the "R" buttons, most recently created first.
    pub reset_list: Vec<u16>,
    /// Remembered dwell values for `updatedwellsliders` (start at 0).
    pub dwell_prev: [f32; 5],
    /// Colour panel mode: showing hue sliders (`true`) or lightness.
    pub hue_mode: bool,
}

impl Controls {
    /// Build every panel in the original's creation order (RGB, lit,
    /// tmesh, z-buffered build, no `.rgb` textures).
    pub fn new() -> Controls {
        build_panels(Controls {
            acts: vec![None; ID_LIMIT],
            sliders: Vec::new(),
            reset_list: Vec::new(),
            dwell_prev: [0.0; 5],
            hue_mode: true,
        })
    }

    pub fn get(&self, id: u16) -> Option<&Actuator> {
        self.acts.get(usize::from(id)).and_then(|a| a.as_ref())
    }

    pub(crate) fn get_mut(&mut self, id: u16) -> Option<&mut Actuator> {
        self.acts.get_mut(usize::from(id)).and_then(|a| a.as_mut())
    }

    /// The actuator with this id; use for ids the code itself names.
    ///
    /// # Panics
    ///
    /// For an unused id.
    pub fn act(&self, id: u16) -> &Actuator {
        self.get(id).unwrap_or_else(|| panic!("no actuator {id}"))
    }

    pub(crate) fn act_mut(&mut self, id: u16) -> &mut Actuator {
        self.get_mut(id)
            .unwrap_or_else(|| panic!("no actuator {id}"))
    }

    /// All actuators in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Actuator> {
        self.acts.iter().flatten()
    }

    /// The value an ep slider contributes to drawing: its value bar's
    /// `extval`. `id` is the slider's main id.
    pub fn slider_value(&self, id: u16) -> f32 {
        self.act(id + ids::SLIDER_VALUE).extval
    }

    /// The ep slider an id belongs to (main or sub-actuator).
    pub fn slider_of(&self, id: u16) -> Option<&EpSlider> {
        self.get(id)
            .and_then(|a| a.slider)
            .map(|i| &self.sliders[i])
    }

    /// Find the actuator for a key equivalent (letters are case-insensitive;
    /// ESC is `'\x1b'`). Only actuators added to a panel have keys.
    pub fn key_target(&self, key: char) -> Option<u16> {
        let k = key.to_ascii_uppercase();
        self.iter()
            .find(|a| a.key == Some(k) && a.panel.is_some())
            .map(|a| a.id)
    }

    /// `setact`'s value step for an ep slider: put the value bar at `v`
    /// clamped into the animation window, and mirror it into `val`.
    pub(crate) fn set_slider_bar(&mut self, s: usize, v: f32) {
        let sl = &self.sliders[s];
        let (main, up, lo, val) = (sl.id, sl.upper(), sl.lower(), sl.value());
        let a = self.act(up).extval;
        let b = self.act(lo).extval;
        let x = clamp_span(v, a, b);
        self.act_mut(val).extval = x;
        self.act_mut(main).val = x;
    }

    /// `saverange`: remember the window's width and centre.
    pub(crate) fn save_range(&mut self, s: usize) {
        let (up, lo) = (self.sliders[s].upper(), self.sliders[s].lower());
        let a = self.act(up).extval;
        let b = self.act(lo).extval;
        let range = a - b;
        let sl = &mut self.sliders[s];
        sl.range = range;
        sl.center = a - range * 0.5;
    }

    /// `setcenter` (lower bound dragged): keep the saved width, clamp the
    /// upper bound to the slider maximum, clamp the value into the window.
    pub(crate) fn set_center(&mut self, s: usize) {
        let a = self.act(self.sliders[s].lower()).extval + self.sliders[s].range;
        self.reclamp_window(s, a);
    }

    /// `setrange` (rate changed): recompute the upper bound from the saved
    /// centre; numerically a re-clamp.
    pub(crate) fn set_range(&mut self, s: usize) {
        let a = self.sliders[s].center * 2.0 - self.act(self.sliders[s].lower()).extval;
        self.reclamp_window(s, a);
    }

    fn reclamp_window(&mut self, s: usize, a: f32) {
        let sl = self.sliders[s];
        let max = self.act(sl.id).max;
        let b = self.act(sl.lower()).extval;
        let a = clamp_span(a, max, b);
        self.act_mut(sl.upper()).extval = a;
        let v = self.act(sl.value()).extval;
        self.act_mut(sl.value()).extval = clamp_span(v, a, b);
        self.save_range(s);
    }

    /// `resetslider` (the slider's "R"): bounds back to the slider range and
    /// rate to its initial value; then, if the value is already at its
    /// initial value, return the accumulator to reset (second stage),
    /// otherwise move the value bar back. `M.val` is not updated.
    pub(crate) fn reset_slider(&mut self, s: usize) -> Option<Accumulator> {
        let sl = self.sliders[s];
        let (min, max, init) = {
            let m = self.act(sl.id);
            (m.min, m.max, m.initval)
        };
        self.act_mut(sl.upper()).extval = max;
        self.act_mut(sl.lower()).extval = min;
        let r = self.act_mut(sl.rate());
        r.val = r.initval;
        let v = self.act_mut(sl.value());
        if init == v.extval {
            sl.accumulator
        } else {
            v.extval = init;
            None
        }
    }

    /// `resetact` (a dial's "R"): the dial back to its initial value. The
    /// dials have no accumulator, so the second stage does nothing.
    pub(crate) fn reset_dial(&mut self, dial: u16) {
        let d = self.act_mut(dial);
        if d.initval != d.val {
            d.val = d.initval;
        }
    }

    /// `animateslider`: one step of a slider's self-oscillation. `speed` is
    /// the speed slider's value as read at this point; `grabbed` is the
    /// actuator the user is holding, which freezes its own slider.
    pub(crate) fn animate_slider(&mut self, s: usize, speed: f32, grabbed: Option<u16>) {
        let sl = self.sliders[s];
        let rate = self.act(sl.rate()).val;
        if grabbed == Some(sl.value()) || rate == 0.0 {
            return;
        }
        let (min, max) = (self.act(sl.id).min, self.act(sl.id).max);
        let a = self.act(sl.upper()).extval;
        let b = self.act(sl.lower()).extval;
        let mut bar =
            self.act(sl.value()).extval + ((sl.dir * rate * (a - b)) / (max - min)) * speed;
        if self.act(sl.bounce()).on() {
            if bar < b {
                bar = b * 2.0 - bar;
                self.sliders[s].dir = 1.0;
            } else if a < bar {
                bar = a * 2.0 - bar;
                self.sliders[s].dir = -1.0;
            }
        } else if bar < b {
            bar = a - (b - bar);
        } else if a < bar {
            bar = b + (bar - a);
        }
        let bar = clamp_span(bar, a, b);
        self.act_mut(sl.value()).extval = bar;
        self.act_mut(sl.id).val = bar;
    }

    /// `chooseupdates` for actuator `id` (colour panel side effects).
    pub(crate) fn choose_updates(&mut self, id: u16) {
        for i in 0..5 {
            if id == ids::dwell(i) {
                self.update_dwell_sliders(i);
            }
            if id == ids::active_button(i) {
                self.update_active_button(i);
            }
        }
    }

    /// `updatedwellsliders`: renormalise the other four dwell values after
    /// dwell slider `k` moved since the last call.
    fn update_dwell_sliders(&mut self, k: usize) {
        let vals: [f32; 5] = std::array::from_fn(|i| self.act(ids::dwell(i)).val);
        let mut sum = 0.0f32;
        for (i, v) in vals.iter().enumerate() {
            if i != k {
                sum += v;
            }
        }
        let delta = vals[k] - self.dwell_prev[k];
        for i in (0..5).filter(|&i| i != k) {
            let a = self.act_mut(ids::dwell(i));
            a.val = if f64::from(sum) == 0.0 {
                (f64::from(a.val) + f64::from(-delta) * 0.2) as f32
            } else {
                a.val + (-delta * a.val) / sum
            };
            a.val = a.val.clamp(0.0, 1.0);
        }
        for i in 0..5 {
            self.dwell_prev[i] = self.act(ids::dwell(i)).val;
        }
    }

    /// `updateactivebutton`: release every other channel's active button.
    fn update_active_button(&mut self, k: usize) {
        for i in (0..5).filter(|&i| i != k) {
            let a = self.act_mut(ids::active_button(i));
            a.val = a.min;
        }
    }

    /// `togglechanmode`: the hue palette switches to lightness mode, the
    /// lightness palette back to hue mode (visibility only).
    pub(crate) fn toggle_chan_mode(&mut self, from: u16) {
        let to_light = from == ids::HUE_PALETTE;
        self.hue_mode = !to_light;
        self.act_mut(ids::HUE_PALETTE).visible = !to_light;
        self.act_mut(ids::LIGHT_PALETTE).visible = to_light;
        for i in 0..5 {
            for (base, show) in [
                (ids::hue_slider(i), !to_light),
                (ids::light_slider(i), to_light),
            ] {
                for off in [0, ids::SLIDER_RATE, ids::SLIDER_BOUNCE] {
                    self.act_mut(base + off).visible = show;
                }
            }
        }
    }
}

impl Default for Controls {
    fn default() -> Self {
        Controls::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_script_comments() {
        let c = Controls::new();
        assert_eq!(c.act(29).label, Some("full screen (F)"));
        assert_eq!(c.act(17).label, Some("auto"));
        assert_eq!(c.act(18).label, Some("outline (O)"));
        for (id, l) in [
            (31, "zoom"),
            (38, "twst"),
            (52, "n"),
            (73, "whl"),
            (80, "spn"),
            (87, "flp"),
            (94, "arm"),
            (101, "wrst"),
            (108, "sz"),
            (318, "t"),
            (339, "T"),
        ] {
            assert_eq!(c.act(id).label, Some(l));
            assert_eq!(c.act(id).kind, Kind::Slider);
        }
        assert_eq!(c.act(127).kind, Kind::SliderRate);
        assert_eq!(c.act(127).val, 0.01);
        assert_eq!(c.act(123).kind, Kind::Slider);
        assert_eq!(c.act(130).kind, Kind::Slider);
        assert_eq!(c.act(195).kind, Kind::Slider);
        assert_eq!(c.act(202).kind, Kind::Slider);
        assert_eq!(c.act(213).kind, Kind::Button);
        assert_eq!(c.act(214).kind, Kind::HSlider);
        assert!(c.get(115).is_none() && c.get(215).is_none() && c.get(12).is_none());
        assert_eq!(c.iter().filter(|a| a.kind == Kind::Slider).count(), 26);
    }

    #[test]
    fn keys() {
        let c = Controls::new();
        assert_eq!(c.key_target('q'), Some(ids::SMEAR));
        assert_eq!(c.key_target('t'), Some(ids::TRIANGLES));
        assert_eq!(c.key_target('W'), None);
        assert_eq!(c.key_target('\x1b'), Some(ids::EXIT));
    }

    #[test]
    fn slider_window_logic() {
        let mut c = Controls::new();
        let s = c.act(ids::ZOOM).slider.unwrap();
        c.set_slider_bar(s, 0.9);
        assert_eq!(c.slider_value(ids::ZOOM), 0.5);
        c.act_mut(ids::ZOOM + 2).extval = -0.1;
        c.save_range(s);
        assert_eq!(c.sliders[s].range, 0.6);
        c.act_mut(ids::ZOOM + 2).extval = 0.2;
        c.set_center(s);
        // Width kept but the upper bound is clamped to the maximum.
        assert_eq!(c.act(ids::ZOOM + 1).extval, 0.5);
        assert_eq!(c.slider_value(ids::ZOOM), 0.5);
        assert_eq!(c.reset_slider(s), None);
        assert_eq!(c.slider_value(ids::ZOOM), 0.0);
        let w = c.act(ids::WHEEL).slider.unwrap();
        assert_eq!(c.reset_slider(w), Some(Accumulator::Wheel));
    }

    #[test]
    fn hue_self_oscillation_wraps() {
        let mut c = Controls::new();
        let s = c.act(ids::hue_slider(0)).slider.unwrap();
        c.animate_slider(s, 1.0, None);
        assert_eq!(c.slider_value(ids::hue_slider(0)), 1.01);
        c.act_mut(ids::hue_slider(0) + 3).extval = 1.995;
        c.animate_slider(s, 1.0, None);
        assert!((c.slider_value(ids::hue_slider(0)) - 0.005).abs() < 1e-6);
    }

    #[test]
    fn dwell_renormalisation() {
        let mut c = Controls::new();
        c.act_mut(ids::dwell(1)).val = 0.5;
        c.update_dwell_sliders(1);
        // First call: the remembered value is 0, so the whole 0.5 is the
        // change and channel 0 gives it up.
        assert_eq!(c.act(ids::dwell(0)).val, 0.5);
        assert_eq!(c.dwell_prev, [0.5, 0.5, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn clamp_span_orders() {
        assert_eq!(clamp_span(5.0, 1.0, 0.0), 1.0);
        assert_eq!(clamp_span(-5.0, 1.0, 0.0), 0.0);
        assert_eq!(clamp_span(0.5, 0.0, 1.0), 0.5);
        assert_eq!(clamp_span(5.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp_span(f32::NAN, 1.0, 0.0), 0.0);
    }
}
