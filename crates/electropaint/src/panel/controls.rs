//! The panel's widgets and how each is wired to the simulation.

use std::cell::Cell;

use electropaint_core::ids;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, sel};
use objc2_app_kit::{
    NSBox, NSBoxType, NSButton, NSColor, NSControl, NSControlSize, NSFont, NSImage, NSSlider,
    NSTextAlignment, NSTextField, NSTitlePosition, NSView,
};

use super::layout::{into_view, ns};
use super::sync::KNOB_STEP;
use super::{Bind, Control, PanelController};

impl PanelController {
    /// The key equivalent ep gives actuator `id`, if any.
    pub(super) fn key(&self, id: u16) -> Option<char> {
        let show = self.ivars().show.borrow();
        show.sim()
            .actuator(id)
            .and_then(|a| a.key)
            .filter(|k| !k.is_control())
    }

    pub(super) fn range(&self, id: u16) -> (f64, f64) {
        let show = self.ivars().show.borrow();
        let (lo, hi) = show
            .sim()
            .actuator(id)
            .unwrap_or_else(|| panic!("no actuator {id}"))
            .span();
        (f64::from(lo), f64::from(hi))
    }

    /// Wires `control` to the panel (tag, target and action for an
    /// `NSControl`) and returns it as a view.
    pub(super) fn add(
        &self,
        control: &(impl AsRef<NSView> + ?Sized),
        bind: Bind,
    ) -> Retained<NSView> {
        self.add_with(control, bind, false, 0.0)
    }

    /// `add` for a symbol toggle (`tinted`) or a slider (`tolerance`); see
    /// `Control`.
    pub(super) fn add_with(
        &self,
        control: &(impl AsRef<NSView> + ?Sized),
        bind: Bind,
        tinted: bool,
        tolerance: f64,
    ) -> Retained<NSView> {
        let view = into_view(control);
        let mut controls = self.ivars().controls.borrow_mut();
        if let Some(control) = view.downcast_ref::<NSControl>() {
            control.setTag(controls.len() as isize);
            // SAFETY: `changed:` is defined on this class and takes the
            // control; the target is weak, and this controller outlives the
            // panel's controls.
            unsafe {
                control.setTarget(Some(self));
                control.setAction(Some(sel!(changed:)));
            }
        }
        controls.push(Control {
            view: view.clone(),
            bind,
            edited: Cell::new(None),
            tinted,
            shown: Cell::new(None),
            tolerance,
        });
        view
    }

    pub(super) fn slider(
        &self,
        mtm: MainThreadMarker,
        min: f64,
        max: f64,
        bind: Bind,
        width: f64,
    ) -> Retained<NSView> {
        let s = NSSlider::new(mtm);
        s.setMinValue(min);
        s.setMaxValue(max);
        s.setContinuous(true);
        s.setControlSize(NSControlSize::Small);
        s.widthAnchor()
            .constraintEqualToConstant(width)
            .setActive(true);
        self.add_with(&*s, bind, false, KNOB_STEP * (max - min) / width)
    }

    pub(super) fn checkbox(
        &self,
        mtm: MainThreadMarker,
        title: &str,
        bind: Bind,
    ) -> Retained<NSView> {
        // SAFETY: no target or action; `add` sets them.
        let b = unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), None, None, mtm) };
        self.add(&*b, bind)
    }

    pub(super) fn radio(&self, mtm: MainThreadMarker, bind: Bind) -> Retained<NSView> {
        // SAFETY: no target or action; `add` sets them.
        let b = unsafe { NSButton::radioButtonWithTitle_target_action(&ns(""), None, None, mtm) };
        self.add(&*b, bind)
    }

    pub(super) fn button(
        &self,
        mtm: MainThreadMarker,
        title: &str,
        bind: Bind,
    ) -> Retained<NSView> {
        // SAFETY: no target or action; `add` sets them.
        let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
        self.add(&*b, bind)
    }

    /// A borderless SF Symbols button, tinted with the accent colour while
    /// its actuator is on (secondary grey otherwise).
    pub(super) fn symbol_button(
        &self,
        mtm: MainThreadMarker,
        symbol: &str,
        tip: &str,
        bind: Bind,
    ) -> Retained<NSView> {
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &ns(symbol),
            Some(&ns(tip)),
        )
        .unwrap_or_else(|| panic!("no SF Symbol {symbol}"));
        // SAFETY: no target or action; `add` sets them.
        let b = unsafe { NSButton::buttonWithImage_target_action(&image, None, None, mtm) };
        b.setBordered(false);
        b.setToolTip(Some(&ns(tip)));
        self.add_with(&*b, bind, true, 0.0)
    }

    /// The value, low, high and rate sliders of the ep slider whose main
    /// actuator is `id`. The bounds take the value's range.
    pub(super) fn ep_slider(
        &self,
        mtm: MainThreadMarker,
        id: u16,
        value_width: f64,
        width: f64,
    ) -> [Retained<NSView>; 4] {
        let (min, max) = self.range(id);
        let (rmin, rmax) = self.range(id + ids::SLIDER_RATE);
        [
            self.slider(mtm, min, max, Bind::Value(id), value_width),
            self.slider(mtm, min, max, Bind::Value(id + ids::SLIDER_LOWER), width),
            self.slider(mtm, min, max, Bind::Value(id + ids::SLIDER_UPPER), width),
            self.slider(mtm, rmin, rmax, Bind::Value(id + ids::SLIDER_RATE), width),
        ]
    }

    /// The bounce toggle of the ep slider whose main actuator is `id`.
    pub(super) fn bounce_button(&self, mtm: MainThreadMarker, id: u16) -> Retained<NSView> {
        self.symbol_button(
            mtm,
            "arrow.left.arrow.right",
            "Bounce at the ends of the range (off: wrap round)",
            Bind::Press(id + ids::SLIDER_BOUNCE),
        )
    }

    /// The "R" of the ep slider whose main actuator is `id`.
    pub(super) fn reset_button(&self, mtm: MainThreadMarker, id: u16) -> Retained<NSView> {
        self.symbol_button(
            mtm,
            "arrow.counterclockwise",
            "Reset range, rate and value",
            Bind::Press(id + ids::SLIDER_RESET),
        )
    }

    pub(super) fn readout(&self, mtm: MainThreadMarker, bind: Bind) -> Retained<NSView> {
        let t = NSTextField::labelWithString(&ns("0"), mtm);
        t.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            11.0, 0.0,
        )));
        t.setTextColor(Some(&NSColor::secondaryLabelColor()));
        t.setAlignment(NSTextAlignment::Right);
        t.widthAnchor()
            .constraintEqualToConstant(56.0)
            .setActive(true);
        self.add(&*t, bind)
    }

    pub(super) fn swatch(&self, mtm: MainThreadMarker, channel: usize) -> Retained<NSView> {
        let b = NSBox::new(mtm);
        b.setBoxType(NSBoxType::Custom);
        b.setTitlePosition(NSTitlePosition::NoTitle);
        b.setBorderWidth(0.0);
        b.setCornerRadius(4.0);
        b.widthAnchor()
            .constraintEqualToConstant(90.0)
            .setActive(true);
        b.heightAnchor()
            .constraintEqualToConstant(16.0)
            .setActive(true);
        self.add(&*b, Bind::Swatch(channel))
    }
}
