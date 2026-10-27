//! The control panel: a native floating window with ep's controls.
//!
//! It mirrors ep's "electro-paint" and "colors" panels (libpanel windows on
//! IRIX) control for control: the same actuators, ranges and behaviour,
//! drawn with standard AppKit controls. Every control drives the simulation
//! through the same actuator inputs a libpanel event would, and the panel
//! follows the simulation as the script animates it.
//!
//! An ep slider is seven actuators: the value, the window the random walk
//! roams (low and high bounds), a self-oscillation rate, a bounce/wrap
//! toggle and "R" (reset). Each becomes one row here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ep_core::colour::hls_to_rgb;
use ep_core::{Kind, ids};
use ep_metal::show::Show;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSButtonType, NSColor,
    NSControl, NSControlSize, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSGridCell,
    NSGridView, NSLayoutConstraintOrientation, NSPanel, NSSegmentedControl, NSSlider, NSSliderType,
    NSStackView, NSTabView, NSTabViewItem, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
};

/// After the user moves a control, the panel leaves it alone this long
/// before following the simulation again, so a drag isn't fought.
const EDIT_GRACE: Duration = Duration::from_millis(400);
const REFRESH_HZ: f64 = 20.0;

/// How a control talks to the simulation.
#[derive(Clone, Copy)]
enum Bind {
    /// A slider or dial: drag `id` to the control's value; shows `value(id)`.
    Value(u16),
    /// A toggle or button: press `id`; shows whether it is on.
    Press(u16),
    /// Read-only text showing `value(id)`.
    Readout(u16),
    /// The colour swatch of channel `i`.
    Swatch(usize),
    /// The position puck, one axis.
    Puck(usize),
    /// Recentre the puck.
    PuckCentre,
    /// Hue/lightness switch for the colour channels.
    ChanMode,
}

struct Control {
    view: Retained<NSView>,
    bind: Bind,
    edited: Cell<Option<Instant>>,
}

pub(crate) struct PanelIvars {
    show: Rc<RefCell<Show>>,
    controls: RefCell<Vec<Control>>,
    puck: Cell<[f32; 2]>,
    window: RefCell<Option<Retained<NSPanel>>>,
    /// `--controls`: keep the panel up regardless of ep's full-screen state.
    pinned: bool,
    /// Grid rows for the hue sliders and the lightness sliders (shown one
    /// set at a time, like ep's two overlapping slider sets).
    chan_rows: RefCell<Option<(Retained<NSGridView>, Vec<isize>, Vec<isize>)>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "EPPanelController"]
    #[ivars = PanelIvars]
    pub(crate) struct PanelController;

    impl PanelController {
        #[unsafe(method(changed:))]
        fn changed(&self, sender: &NSControl) {
            self.on_change(sender.tag() as usize);
        }

        #[unsafe(method(refresh:))]
        fn refresh_timer(&self, _timer: &NSTimer) {
            self.refresh();
        }
    }

    unsafe impl NSObjectProtocol for PanelController {}

    unsafe impl NSWindowDelegate for PanelController {
        /// Closing the panel is ep's "full screen" again, which hides the
        /// panels; a pinned panel just unpins.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &AnyObject) -> bool {
            let mut show = self.ivars().show.borrow_mut();
            if show.sim().panels_visible() {
                show.sim_mut().press(ids::FULLSCREEN);
            }
            true
        }
    }
);

impl PanelController {
    /// Builds the panel. It appears when ep's panels would (outside full
    /// screen), or straight away if `pinned`. `level` is the window level of
    /// the show, which the panel must float above.
    pub(crate) fn new(
        mtm: MainThreadMarker,
        show: Rc<RefCell<Show>>,
        pinned: bool,
        level: isize,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PanelIvars {
            show,
            controls: RefCell::new(Vec::new()),
            puck: Cell::new([0.0; 2]),
            window: RefCell::new(None),
            pinned,
            chan_rows: RefCell::new(None),
            timer: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        let tabs = NSTabView::new(mtm);
        let main = this.main_tab(mtm);
        let colours = this.colours_tab(mtm);
        for (label, view) in [("electro-paint", main), ("colors", colours)] {
            let item = NSTabViewItem::new();
            item.setLabel(&NSString::from_str(label));
            item.setView(Some(&view));
            tabs.addTabViewItem(&item);
        }
        let size = tabs.fittingSize();
        let size = NSSize::new(size.width.max(720.0), size.height.max(560.0));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), size),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::UtilityWindow,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setTitle(&ns("electropaint controls"));
        panel.setContentView(Some(&tabs));
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setLevel(level + 1);
        panel.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        panel.center();
        *this.ivars().window.borrow_mut() = Some(panel);

        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                1.0 / REFRESH_HZ,
                &this,
                sel!(refresh:),
                None,
                true,
            )
        };
        *this.ivars().timer.borrow_mut() = Some(timer);
        this.refresh();
        this
    }

    /// Draws each tab of the panel into `<path>-<tab>.png` (for checking
    /// the layout without a screen).
    pub(crate) fn snapshot(&self, path: &str) -> Result<(), String> {
        let window = self.ivars().window.borrow();
        let window = window.as_ref().ok_or("no panel")?;
        let tabs = window.contentView().ok_or("no content")?;
        let tabs = tabs.downcast_ref::<NSTabView>().ok_or("not a tab view")?;
        self.refresh_values();
        for i in 0..tabs.numberOfTabViewItems() {
            tabs.selectTabViewItemAtIndex(i);
            tabs.layoutSubtreeIfNeeded();
            let rect = tabs.bounds();
            let rep = tabs
                .bitmapImageRepForCachingDisplayInRect(rect)
                .ok_or("no bitmap")?;
            tabs.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
            let png = unsafe {
                rep.representationUsingType_properties(
                    objc2_app_kit::NSBitmapImageFileType::PNG,
                    &objc2_foundation::NSDictionary::new(),
                )
            }
            .ok_or("no png")?;
            let file = format!("{path}-{i}.png");
            std::fs::write(&file, png.to_vec()).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // ------------------------------------------------------------- layout

    fn main_tab(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let toggles: [(u16, &str); 14] = [
            (ids::OUTLINE, "outline (O)"),
            (ids::FATLINE, "fat lines (I)"),
            (ids::FILL, "fill (U)"),
            (ids::RIBBON, "ribbons (R)"),
            (ids::SMOOTH, "smooth (M)"),
            (ids::SMEAR, "smear (Q)"),
            // ep creates this toggle but leaves it off its RGB panel (and so
            // without a key); scripts can still reach it, and so can this panel.
            (ids::FADE, "fade"),
            (ids::BACKGROUND, "back (E)"),
            (ids::ZBUF, "z-buf (Z)"),
            (ids::LIGHT, "light (L)"),
            (ids::STOP, "stop (S)"),
            (ids::FULLSCREEN, "full screen (F)"),
            (ids::AUTO, "auto"),
            // From ep's texture panel, which the port doesn't have.
            (ids::TRIANGLES, "triangles (T)"),
        ];
        let mut toggle_rows: Vec<Vec<Retained<NSView>>> = Vec::new();
        for chunk in toggles.chunks(4) {
            let mut row: Vec<Retained<NSView>> = chunk
                .iter()
                .map(|&(id, label)| self.checkbox(mtm, label, Bind::Press(id)))
                .collect();
            while row.len() < 4 {
                row.push(empty(mtm));
            }
            toggle_rows.push(row);
        }
        let buttons = hstack(
            mtm,
            &[
                self.button(mtm, "reset", Bind::Press(ids::RESET)),
                self.button(mtm, "exit", Bind::Press(ids::EXIT)),
            ],
        );
        let toggles: Retained<NSView> = Retained::into_super(grid(mtm, toggle_rows));

        let mut rows = vec![slider_header(mtm)];
        for (id, label) in [
            (ids::ZOOM, "zoom"),
            (ids::TWIST, "twst"),
            (ids::SPEED, "spd"),
            (ids::N, "n"),
            (ids::AMBIENT, "amb"),
            (ids::SHINY, "shn"),
            (ids::WHEEL, "whl"),
            (ids::SPIN, "spn"),
            (ids::FLIP, "flp"),
            (ids::ARM, "arm"),
            (ids::WRIST, "wrst"),
            (ids::SIZE, "sz"),
        ] {
            rows.push(self.ep_slider_row(mtm, label, id, 200.0));
        }
        let sliders: Retained<NSView> = Retained::into_super(grid(mtm, rows));

        let puck = hstack(
            mtm,
            &[
                label(mtm, "position"),
                label(mtm, "x"),
                self.slider(mtm, -0.5, 0.5, Bind::Puck(0), 140.0),
                label(mtm, "y"),
                self.slider(mtm, -0.5, 0.5, Bind::Puck(1), 140.0),
                self.button(mtm, "centre", Bind::PuckCentre),
            ],
        );
        let credit = label(mtm, "...by David Tristram");
        vstack(mtm, &[toggles, buttons, sliders, puck, credit])
    }

    fn colours_tab(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let mut dials: Vec<Retained<NSView>> = Vec::new();
        for (id, text) in [
            (ids::ALPHA_OUT, "outa"),
            (ids::ALPHA_BACK, "baka"),
            (ids::HUE_BACK_RATE, "outr"),
        ] {
            let (min, max) = self.range(id);
            dials.push(label(mtm, text));
            dials.push(self.dial(mtm, min, max, Bind::Value(id)));
            dials.push(self.small_button(mtm, "R", Bind::Press(id + 1)));
        }
        let dials = hstack(mtm, &dials);

        let mode = NSSegmentedControl::new(mtm);
        mode.setSegmentCount(2);
        mode.setLabel_forSegment(&ns("hue"), 0);
        mode.setLabel_forSegment(&ns("lightness"), 1);
        let mode: Retained<NSView> = Retained::into_super(Retained::into_super(mode));
        self.register(&mode, Bind::ChanMode);
        let mode = hstack(mtm, &[label(mtm, "channel sliders"), mode]);

        // One column per colour channel.
        let channels = 0..5;
        let header = std::iter::once(empty(mtm))
            .chain(
                channels
                    .clone()
                    .map(|i| label(mtm, &format!("channel {}", i + 1))),
            )
            .collect();
        let row = |name: &str, make: &dyn Fn(usize) -> Retained<NSView>| -> Vec<Retained<NSView>> {
            std::iter::once(label(mtm, name))
                .chain(channels.clone().map(make))
                .collect()
        };
        let mut rows: Vec<Vec<Retained<NSView>>> = vec![
            header,
            row("active", &|i| {
                self.button(mtm, "on", Bind::Press(ids::active_button(i)))
            }),
            row("colour", &|i| self.swatch(mtm, i)),
            row("alpha", &|i| {
                self.dial(mtm, 0.0, 1.0, Bind::Value(ids::alpha_dial(i)))
            }),
        ];
        let mut hue_rows = Vec::new();
        let mut light_rows = Vec::new();
        for (set, base) in [
            (&mut hue_rows, ids::hue_slider as fn(usize) -> u16),
            (&mut light_rows, ids::light_slider),
        ] {
            for (name, offset) in [
                ("value", None),
                ("low", Some(ids::SLIDER_LOWER)),
                ("high", Some(ids::SLIDER_UPPER)),
                ("rate", Some(ids::SLIDER_RATE)),
            ] {
                set.push(rows.len() as isize);
                rows.push(row(name, &|i| {
                    let id = offset.map_or(base(i), |o| base(i) + o);
                    let (min, max) = self.range(id);
                    self.slider(mtm, min, max, Bind::Value(id), 100.0)
                }));
            }
            set.push(rows.len() as isize);
            rows.push(row("", &|i| {
                hstack(
                    mtm,
                    &[
                        self.checkbox(mtm, "bounce", Bind::Press(base(i) + ids::SLIDER_BOUNCE)),
                        self.small_button(mtm, "R", Bind::Press(base(i) + ids::SLIDER_RESET)),
                    ],
                )
            }));
        }
        rows.push(row("dwell", &|i| {
            self.slider(mtm, 0.0, 1.0, Bind::Value(ids::dwell(i)), 100.0)
        }));
        let grid_view = grid(mtm, rows);
        *self.ivars().chan_rows.borrow_mut() = Some((grid_view.clone(), hue_rows, light_rows));

        let (min, max) = self.range(ids::CHAN_RATE);
        let chanrate = hstack(
            mtm,
            &[
                label(mtm, "channel rate"),
                self.slider(mtm, min, max, Bind::Value(ids::CHAN_RATE), 300.0),
            ],
        );
        let grid_view: Retained<NSView> = Retained::into_super(grid_view);
        vstack(mtm, &[dials, mode, grid_view, chanrate])
    }

    /// One ep slider: value, bounds, rate, bounce, R.
    fn ep_slider_row(
        &self,
        mtm: MainThreadMarker,
        name: &str,
        id: u16,
        width: f64,
    ) -> Vec<Retained<NSView>> {
        let (min, max) = self.range(id);
        let (rmin, rmax) = self.range(id + ids::SLIDER_RATE);
        vec![
            label(mtm, name),
            self.slider(mtm, min, max, Bind::Value(id), width),
            self.readout(mtm, Bind::Readout(id)),
            self.slider(mtm, min, max, Bind::Value(id + ids::SLIDER_LOWER), 80.0),
            self.slider(mtm, min, max, Bind::Value(id + ids::SLIDER_UPPER), 80.0),
            self.slider(mtm, rmin, rmax, Bind::Value(id + ids::SLIDER_RATE), 80.0),
            self.checkbox(mtm, "", Bind::Press(id + ids::SLIDER_BOUNCE)),
            self.small_button(mtm, "R", Bind::Press(id + ids::SLIDER_RESET)),
        ]
    }

    // ----------------------------------------------------------- controls

    fn range(&self, id: u16) -> (f64, f64) {
        let show = self.ivars().show.borrow();
        let a = show.sim().actuator(id).expect("actuator exists");
        let (lo, hi) = (a.min.min(a.max), a.min.max(a.max));
        (f64::from(lo), f64::from(hi))
    }

    fn register(&self, view: &Retained<NSView>, bind: Bind) {
        let mut controls = self.ivars().controls.borrow_mut();
        if let Some(control) = view.downcast_ref::<NSControl>() {
            control.setTag(controls.len() as isize);
            unsafe {
                control.setTarget(Some(self));
                control.setAction(Some(sel!(changed:)));
            }
        }
        controls.push(Control {
            view: view.clone(),
            bind,
            edited: Cell::new(None),
        });
    }

    fn slider(
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
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(s));
        self.register(&view, bind);
        view
    }

    fn dial(&self, mtm: MainThreadMarker, min: f64, max: f64, bind: Bind) -> Retained<NSView> {
        let s = NSSlider::new(mtm);
        s.setSliderType(NSSliderType::Circular);
        s.setMinValue(min);
        s.setMaxValue(max);
        s.setContinuous(true);
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(s));
        self.register(&view, bind);
        view
    }

    fn checkbox(&self, mtm: MainThreadMarker, title: &str, bind: Bind) -> Retained<NSView> {
        let b = unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), None, None, mtm) };
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(b));
        self.register(&view, bind);
        view
    }

    fn button(&self, mtm: MainThreadMarker, title: &str, bind: Bind) -> Retained<NSView> {
        let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
        if let Bind::Press(id) = bind {
            // Momentary buttons that light up (the channel "active" buttons)
            // show their state.
            let lights = self
                .ivars()
                .show
                .borrow()
                .sim()
                .actuator(id)
                .is_some_and(|a| a.kind == Kind::Button);
            if lights {
                b.setButtonType(NSButtonType::PushOnPushOff);
            }
        }
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(b));
        self.register(&view, bind);
        view
    }

    fn small_button(&self, mtm: MainThreadMarker, title: &str, bind: Bind) -> Retained<NSView> {
        let view = self.button(mtm, title, bind);
        if let Some(b) = view.downcast_ref::<NSButton>() {
            b.setControlSize(NSControlSize::Small);
        }
        view
    }

    fn readout(&self, mtm: MainThreadMarker, bind: Bind) -> Retained<NSView> {
        let t = NSTextField::labelWithString(&ns("0"), mtm);
        t.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            11.0, 0.0,
        )));
        t.widthAnchor()
            .constraintEqualToConstant(64.0)
            .setActive(true);
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(t));
        self.register(&view, bind);
        view
    }

    fn swatch(&self, mtm: MainThreadMarker, channel: usize) -> Retained<NSView> {
        let b = NSBox::new(mtm);
        b.setBoxType(NSBoxType::Custom);
        b.setBorderWidth(0.0);
        b.setTitle(&ns(""));
        b.widthAnchor()
            .constraintEqualToConstant(100.0)
            .setActive(true);
        b.heightAnchor()
            .constraintEqualToConstant(20.0)
            .setActive(true);
        let view: Retained<NSView> = Retained::into_super(b);
        self.register(&view, Bind::Swatch(channel));
        view
    }

    // ------------------------------------------------------------ events

    fn on_change(&self, index: usize) {
        let controls = self.ivars().controls.borrow();
        let Some(c) = controls.get(index) else { return };
        c.edited.set(Some(Instant::now()));
        let mut show = self.ivars().show.borrow_mut();
        let sim = show.sim_mut();
        match c.bind {
            Bind::Value(id) => {
                if let Some(s) = c.view.downcast_ref::<NSControl>() {
                    sim.set_value(id, s.doubleValue() as f32);
                }
            }
            Bind::Press(id) => sim.press(id),
            Bind::Puck(axis) => {
                if let Some(s) = c.view.downcast_ref::<NSControl>() {
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
                let lightness = !sim.actuator(ids::HUE_PALETTE).is_some_and(|a| a.visible);
                let want = c
                    .view
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
            Bind::Readout(_) | Bind::Swatch(_) => {}
        }
    }

    /// Follows the simulation: values, toggle states, swatches, which
    /// channel sliders are showing, and whether the panel is up at all.
    fn refresh(&self) {
        let mtm = MainThreadMarker::from(self);
        let show = self.ivars().show.borrow();
        let sim = show.sim();
        if sim.exited() {
            drop(show);
            NSApplication::sharedApplication(mtm).terminate(None);
            return;
        }
        let window_ref = self.ivars().window.borrow();
        let Some(window) = window_ref.as_ref() else {
            return;
        };
        let want_visible = self.ivars().pinned || sim.panels_visible();
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
        drop(window_ref);
        drop(show);
        self.refresh_values();
    }

    /// Sets every control from the simulation (skipping ones just edited).
    fn refresh_values(&self) {
        let show = self.ivars().show.borrow();
        let sim = show.sim();
        let now = Instant::now();
        for c in self.ivars().controls.borrow().iter() {
            if c.edited.get().is_some_and(|t| now - t < EDIT_GRACE) {
                continue;
            }
            match c.bind {
                Bind::Value(id) => {
                    if let (Some(s), Some(v)) = (c.view.downcast_ref::<NSControl>(), sim.value(id))
                    {
                        s.setDoubleValue(f64::from(v));
                    }
                }
                Bind::Press(id) => {
                    if let Some(b) = c.view.downcast_ref::<NSButton>() {
                        let on = sim.is_on(id);
                        b.setState(if on {
                            NSControlStateValueOn
                        } else {
                            NSControlStateValueOff
                        });
                    }
                }
                Bind::Readout(id) => {
                    if let (Some(t), Some(v)) =
                        (c.view.downcast_ref::<NSTextField>(), sim.value(id))
                    {
                        t.setStringValue(&NSString::from_str(&format!("{v:.3}")));
                    }
                }
                Bind::Swatch(i) => {
                    if let Some(b) = c.view.downcast_ref::<NSBox>() {
                        let hue = sim.value(ids::hue_slider(i)).unwrap_or(0.0);
                        let light = sim.value(ids::light_slider(i)).unwrap_or(0.5);
                        let [r, g, bl] = hls_to_rgb(f64::from(hue) % 1.0, f64::from(light), 1.0);
                        b.setFillColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
                            f64::from(r),
                            f64::from(g),
                            f64::from(bl),
                            1.0,
                        ));
                    }
                }
                Bind::Puck(axis) => {
                    if let Some(s) = c.view.downcast_ref::<NSControl>() {
                        s.setDoubleValue(f64::from(self.ivars().puck.get()[axis]));
                    }
                }
                Bind::ChanMode => {
                    if let Some(s) = c.view.downcast_ref::<NSSegmentedControl>() {
                        let lightness = !sim.actuator(ids::HUE_PALETTE).is_some_and(|a| a.visible);
                        s.setSelectedSegment(lightness as isize);
                    }
                }
                Bind::PuckCentre => {}
            }
        }
        if let Some((grid, hue, light)) = self.ivars().chan_rows.borrow().as_ref() {
            let lightness = !sim.actuator(ids::HUE_PALETTE).is_some_and(|a| a.visible);
            for (rows, hidden) in [(hue, lightness), (light, !lightness)] {
                for &r in rows {
                    grid.rowAtIndex(r).setHidden(hidden);
                }
            }
        }
    }
}

// ----------------------------------------------------------------- helpers

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSView> {
    let t = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    Retained::into_super(Retained::into_super(t))
}

fn empty(mtm: MainThreadMarker) -> Retained<NSView> {
    NSGridCell::emptyContentView(mtm)
}

fn slider_header(mtm: MainThreadMarker) -> Vec<Retained<NSView>> {
    ["", "value", "", "low", "high", "rate", "bounce", ""]
        .iter()
        .map(|s| label(mtm, s))
        .collect()
}

fn grid(mtm: MainThreadMarker, rows: Vec<Vec<Retained<NSView>>>) -> Retained<NSGridView> {
    let rows: Vec<Retained<NSArray<NSView>>> = rows
        .into_iter()
        .map(|r| NSArray::from_retained_slice(&r))
        .collect();
    let g = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&rows), mtm);
    g.setRowSpacing(4.0);
    g.setColumnSpacing(8.0);
    // Keep rows at their natural height when the tab is taller.
    g.setContentHuggingPriority_forOrientation(999.0, NSLayoutConstraintOrientation::Vertical);
    g
}

fn hstack(mtm: MainThreadMarker, views: &[Retained<NSView>]) -> Retained<NSView> {
    stack(mtm, views, NSUserInterfaceLayoutOrientation::Horizontal)
}

fn vstack(mtm: MainThreadMarker, views: &[Retained<NSView>]) -> Retained<NSView> {
    let v = stack(mtm, views, NSUserInterfaceLayoutOrientation::Vertical);
    if let Some(s) = v.downcast_ref::<NSStackView>() {
        s.setSpacing(14.0);
        s.setEdgeInsets(objc2_foundation::NSEdgeInsets {
            top: 12.0,
            left: 12.0,
            bottom: 12.0,
            right: 12.0,
        });
        s.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
    }
    v
}

fn stack(
    mtm: MainThreadMarker,
    views: &[Retained<NSView>],
    o: NSUserInterfaceLayoutOrientation,
) -> Retained<NSView> {
    let s = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(views), mtm);
    s.setOrientation(o);
    Retained::into_super(s)
}
