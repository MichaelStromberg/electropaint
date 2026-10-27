//! Keeping the panel and the simulation in step: user input goes to the
//! simulation, and a timer brings every control up to date with it.

use std::cell::Cell;
use std::time::{Duration, Instant};

use electropaint_core::colour::hls_to_rgb;
use electropaint_core::{Sim, ids};
use electropaint_metal::surface::Style;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSBox, NSButton, NSColor, NSControl, NSControlStateValueOff, NSControlStateValueOn, NSGridView,
    NSSegmentedControl, NSTextField, NSView,
};
use objc2_foundation::NSString;

use super::{Bind, Control, PanelController};

/// After the user moves a control, the panel leaves it alone this long
/// before following the simulation again, so a drag isn't fought.
const EDIT_GRACE: Duration = Duration::from_millis(400);
pub(super) const REFRESH_HZ: f64 = 20.0;
/// Readouts change every refresh while a value drifts, and every change
/// costs a layout pass; a few updates a second is as fast as they can be read.
const READOUT_EVERY: u64 = 4;
/// A slider is moved only once its knob would move at least this far, in
/// points: setting a slider redraws it even for an invisible change.
pub(super) const KNOB_STEP: f64 = 0.5;

impl Control {
    /// Whether the control can be left alone this refresh, with the
    /// simulation at `state`.
    fn is_current(&self, state: [f32; 2], refresh: u64) -> bool {
        let Some(shown) = self.shown.get() else {
            return false;
        };
        if shown == state {
            return true;
        }
        match self.bind {
            Bind::Readout(_) => !refresh.is_multiple_of(READOUT_EVERY),
            Bind::Value(_) => f64::from((state[0] - shown[0]).abs()) < self.tolerance,
            _ => false,
        }
    }
}

/// The channel grid and its rows for the hue sliders and for the
/// lightness sliders (shown one set at a time, like ep's two overlapping
/// slider sets).
pub(super) struct ChanRows {
    pub(super) grid: Retained<NSGridView>,
    pub(super) hue: Vec<isize>,
    pub(super) light: Vec<isize>,
    /// The mode last shown.
    pub(super) hue_mode: Cell<Option<bool>>,
}

impl ChanRows {
    pub(super) fn show(&self, hue_mode: bool) {
        if self.hue_mode.replace(Some(hue_mode)) == Some(hue_mode) {
            return;
        }
        let lightness = !hue_mode;
        for (rows, hidden) in [(&self.hue, lightness), (&self.light, !lightness)] {
            for &r in rows {
                self.grid.rowAtIndex(r).setHidden(hidden);
            }
        }
    }
}

impl PanelController {
    pub(super) fn on_change(&self, index: usize) {
        let Some((bind, view)) = self.note_edit(index) else {
            return;
        };
        if matches!(bind, Bind::Page) {
            // The panel's own state. The switch animates the window's
            // resize, so it runs with no borrows held.
            if let Some(s) = view.downcast_ref::<NSSegmentedControl>() {
                self.show_page(s.selectedSegment().max(0) as usize, true);
            }
            return;
        }
        if matches!(bind, Bind::Style) {
            if let Some(s) = view.downcast_ref::<NSSegmentedControl>() {
                let style = if s.selectedSegment() == 1 {
                    Style::Remastered
                } else {
                    Style::Classic
                };
                self.ivars().style.set(style);
            }
            return;
        }
        let mut show = self.ivars().show.borrow_mut();
        self.send(show.sim_mut(), bind, &view);
    }

    /// Records that the user moved control `index` and returns what acting on
    /// it needs, letting go of the borrow: acting may run the run loop, and
    /// with it the show and this panel's refresh.
    fn note_edit(&self, index: usize) -> Option<(Bind, Retained<NSView>)> {
        let controls = self.ivars().controls.borrow();
        let c = controls.get(index)?;
        c.edited.set(Some(Instant::now()));
        // Show the simulation's value again afterwards even if that hasn't
        // changed.
        c.shown.set(None);
        if matches!(c.bind, Bind::Press(_)) {
            // A press can change controls nobody touched: AppKit turns off
            // the other radios of a group itself, and a reset moves many
            // sliders. Show them all afresh.
            for other in controls.iter() {
                other.shown.set(None);
            }
        }
        Some((c.bind, c.view.clone()))
    }

    /// Sends the input of control `view`, bound by `bind`, to the simulation.
    fn send(&self, sim: &mut Sim, bind: Bind, view: &NSView) {
        match bind {
            Bind::Value(id) => {
                if let Some(s) = view.downcast_ref::<NSControl>() {
                    sim.set_value(id, s.doubleValue() as f32);
                }
            }
            Bind::Press(id) => sim.press(id),
            Bind::Puck(axis) => {
                if let Some(s) = view.downcast_ref::<NSControl>() {
                    let mut p = self.ivars().puck.get();
                    p[axis] = s.doubleValue() as f32;
                    self.ivars().puck.set(p);
                    sim.set_puck(p[0], p[1]);
                }
            }
            Bind::PuckCentre => {
                self.ivars().puck.set([0.0; 2]);
                sim.set_puck(0.0, 0.0);
            }
            Bind::ChanMode => {
                // Clicking ep's hue palette switches to lightness, and the
                // lightness palette switches back.
                let lightness = !sim.controls().hue_mode;
                let want = view
                    .downcast_ref::<NSSegmentedControl>()
                    .map_or(0, |s| s.selectedSegment());
                if (want == 1) != lightness {
                    sim.press(if lightness {
                        ids::LIGHT_PALETTE
                    } else {
                        ids::HUE_PALETTE
                    });
                }
            }
            Bind::Page | Bind::Style | Bind::Readout(_) | Bind::Swatch(_) => {}
        }
    }

    /// Follows the simulation: values, toggle states, swatches, which
    /// channel sliders are showing, and whether the panel is up at all.
    pub(super) fn refresh(&self) {
        let mtm = MainThreadMarker::from(self);
        let show = self.ivars().show.borrow();
        let sim = show.sim();
        if sim.exited() {
            drop(show);
            crate::quit(mtm);
            return;
        }
        let Some(chrome) = self.ivars().chrome.get() else {
            return;
        };
        let window = &chrome.window;
        let want_visible = self.ivars().pinned.get() || sim.panels_visible();
        if want_visible != window.isVisible() {
            if want_visible {
                window.orderFront(None);
            } else {
                window.orderOut(None);
            }
        }
        if !window.isVisible() {
            return;
        }
        drop(show);
        self.refresh_values();
    }

    /// Sets every control from the simulation (skipping ones just edited).
    pub(super) fn refresh_values(&self) {
        let show = self.ivars().show.borrow();
        let sim = show.sim();
        let now = Instant::now();
        let refresh = self.ivars().refreshes.get();
        self.ivars().refreshes.set(refresh + 1);
        for c in self.ivars().controls.borrow().iter() {
            if c.edited.get().is_some_and(|t| now - t < EDIT_GRACE) {
                continue;
            }
            let state = self.state_of(c, sim);
            if c.is_current(state, refresh) {
                continue;
            }
            c.shown.set(Some(state));
            show_control(c, state);
        }
        if let Some(rows) = self.ivars().chan_rows.borrow().as_ref() {
            rows.show(sim.controls().hue_mode);
        }
    }

    /// What `c` shows of the simulation: when this is unchanged, so is the
    /// control. (A missing value reads as NaN, which never compares equal.)
    fn state_of(&self, c: &Control, sim: &Sim) -> [f32; 2] {
        let flag = |b: bool| f32::from(u8::from(b));
        match c.bind {
            Bind::Value(id) | Bind::Readout(id) => [sim.value(id).unwrap_or(f32::NAN), 0.0],
            Bind::Press(id) => [flag(sim.is_on(id)), 0.0],
            Bind::Swatch(i) => [
                sim.value(ids::hue_slider(i)).unwrap_or(f32::NAN),
                sim.value(ids::light_slider(i)).unwrap_or(f32::NAN),
            ],
            Bind::Puck(axis) => [self.ivars().puck.get()[axis], 0.0],
            Bind::ChanMode => [flag(sim.controls().hue_mode), 0.0],
            Bind::Style => [flag(self.ivars().style.get() == Style::Remastered), 0.0],
            Bind::PuckCentre | Bind::Page => [0.0; 2],
        }
    }
}

/// Sets control `c` to show `state`, as computed by `state_of`.
fn show_control(c: &Control, state: [f32; 2]) {
    let [a, b] = state;
    let view = &*c.view;
    match c.bind {
        Bind::Value(_) | Bind::Puck(_) => show_number(view, a),
        Bind::Press(_) => {
            if let Some(button) = view.downcast_ref::<NSButton>() {
                show_on(button, a != 0.0, c.tinted);
            }
        }
        Bind::Readout(_) => show_text(view, a),
        Bind::Swatch(_) => {
            if let Some(swatch) = view.downcast_ref::<NSBox>() {
                swatch.setFillColor(&swatch_colour(nan_or(a, 0.0), nan_or(b, 0.5)));
            }
        }
        Bind::ChanMode => {
            if let Some(s) = view.downcast_ref::<NSSegmentedControl>() {
                // Hue mode shows segment 0, lightness segment 1.
                s.setSelectedSegment(isize::from(a == 0.0));
            }
        }
        Bind::Style => {
            if let Some(s) = view.downcast_ref::<NSSegmentedControl>() {
                s.setSelectedSegment(isize::from(a != 0.0));
            }
        }
        Bind::PuckCentre | Bind::Page => {}
    }
}

/// A slider's value, unless the simulation has none (NaN).
fn show_number(view: &NSView, v: f32) {
    if let Some(s) = view.downcast_ref::<NSControl>()
        && !v.is_nan()
    {
        s.setDoubleValue(f64::from(v));
    }
}

/// A readout's text, unless the simulation has no value (NaN).
fn show_text(view: &NSView, v: f32) {
    if let Some(t) = view.downcast_ref::<NSTextField>()
        && !v.is_nan()
    {
        t.setStringValue(&NSString::from_str(&format!("{v:.3}")));
    }
}

/// `v`, or `default` for a missing value (NaN).
fn nan_or(v: f32, default: f32) -> f32 {
    if v.is_nan() { default } else { v }
}

/// A toggle's state; a `tinted` symbol button also takes the accent colour
/// while on.
fn show_on(button: &NSButton, on: bool, tinted: bool) {
    button.setState(if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    if tinted {
        let tint = if on {
            NSColor::controlAccentColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        button.setContentTintColor(Some(&tint));
    }
}

/// A colour channel's swatch: its hue at full saturation and its lightness.
fn swatch_colour(hue: f32, light: f32) -> Retained<NSColor> {
    let [r, g, b] = hls_to_rgb(f64::from(hue) % 1.0, f64::from(light), 1.0);
    NSColor::colorWithSRGBRed_green_blue_alpha(f64::from(r), f64::from(g), f64::from(b), 1.0)
}
