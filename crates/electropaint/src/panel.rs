//! The control panel: a native floating window with ep's controls.
//!
//! It offers everything on ep's "electro-paint" and "colors" panels
//! (libpanel windows on IRIX): the same actuators, ranges and behaviour,
//! laid out as a macOS settings window of titled sections. Every control
//! drives the simulation through the same actuator inputs a libpanel event
//! would, and the panel follows the simulation as the script animates it.
//!
//! An ep slider is seven actuators: the value, the window the random walk
//! roams (low and high bounds), a self-oscillation rate, a bounce/wrap
//! toggle and "R" (reset). Each becomes one row here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ep_core::colour::hls_to_rgb;
use ep_core::{Sim, ids};
use ep_metal::show::Show;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSApplication,
    NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSControl, NSControlSize,
    NSControlStateValueOff, NSControlStateValueOn, NSFont, NSGridCell, NSGridCellPlacement,
    NSGridRowAlignment, NSGridView, NSImage, NSLayoutAttribute, NSLayoutConstraint,
    NSLayoutConstraintOrientation, NSPanel, NSSegmentedControl, NSSlider, NSStackView,
    NSStackViewGravity, NSTextAlignment, NSTextField, NSTitlePosition,
    NSUserInterfaceLayoutOrientation, NSView, NSWindowDelegate, NSWindowOrderingMode,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
};

/// After the user moves a control, the panel leaves it alone this long
/// before following the simulation again, so a drag isn't fought.
const EDIT_GRACE: Duration = Duration::from_millis(400);
const REFRESH_HZ: f64 = 20.0;
/// Readouts change every refresh while a value drifts, and every change
/// costs a layout pass; a few updates a second is as fast as they can be read.
const READOUT_EVERY: u64 = 4;
/// A slider is moved only once its knob would move at least this far, in
/// points: setting a slider redraws it even for an invisible change.
const KNOB_STEP: f64 = 0.5;
/// Space around the window's content and between its parts.
const MARGIN: f64 = 20.0;

/// The ep sliders on the shape page, with full names for ep's four-letter
/// labels.
const SLIDERS: [(u16, &str); 12] = [
    (ids::ZOOM, "Zoom"),
    (ids::TWIST, "Twist"),
    (ids::SPEED, "Speed"),
    (ids::N, "Length"),
    (ids::AMBIENT, "Ambient"),
    (ids::SHINY, "Shininess"),
    (ids::WHEEL, "Wheel"),
    (ids::SPIN, "Spin"),
    (ids::FLIP, "Flip"),
    (ids::ARM, "Arm"),
    (ids::WRIST, "Wrist"),
    (ids::SIZE, "Size"),
];
const TILES: [(u16, &str); 5] = [
    (ids::OUTLINE, "Outline"),
    (ids::FATLINE, "Fat lines"),
    (ids::FILL, "Fill"),
    (ids::TRIANGLES, "Triangles"),
    (ids::RIBBON, "Ribbons"),
];
const EFFECTS: [(u16, &str); 6] = [
    (ids::SMEAR, "Smear"),
    (ids::SMOOTH, "Smooth"),
    // ep creates this toggle but leaves it off its RGB panel (and so without
    // a key); scripts can still reach it, and so can this panel.
    (ids::FADE, "Fade"),
    (ids::BACKGROUND, "Background"),
    (ids::ZBUF, "Z-buffer"),
    (ids::LIGHT, "Lighting"),
];
const PLAYBACK: [(u16, &str); 3] = [
    (ids::STOP, "Stop"),
    (ids::FULLSCREEN, "Full screen"),
    (ids::AUTO, "Random drift"),
];
/// ep's three colour-panel dials (outa, baka, outr).
const BACKDROP: [(u16, &str); 3] = [
    (ids::ALPHA_OUT, "Outline opacity"),
    (ids::ALPHA_BACK, "Background opacity"),
    (ids::HUE_BACK_RATE, "Background hue rate"),
];

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
    /// Which page of the panel is showing (the panel's own state).
    Page,
}

struct Control {
    view: Retained<NSView>,
    bind: Bind,
    edited: Cell<Option<Instant>>,
    /// A symbol button standing for a toggle: tinted while it is on.
    tinted: bool,
    /// The simulation state last shown (see `state_of`). Setting a control
    /// costs AppKit a layout pass and a redraw even when nothing changes, so
    /// unchanged controls are left alone.
    shown: Cell<Option<[f32; 2]>>,
    /// How far a slider's value may drift from `shown` before it is set
    /// again: `KNOB_STEP` of knob travel. 0 for other controls.
    tolerance: f64,
}

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
struct ChanRows {
    grid: Retained<NSGridView>,
    hue: Vec<isize>,
    light: Vec<isize>,
    /// The mode last shown.
    hue_mode: Cell<Option<bool>>,
}

impl ChanRows {
    fn show(&self, hue_mode: bool) {
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

pub(crate) struct PanelIvars {
    show: Rc<RefCell<Show>>,
    controls: RefCell<Vec<Control>>,
    puck: Cell<[f32; 2]>,
    window: RefCell<Option<Retained<NSPanel>>>,
    /// The window's content: the page picker, the slot holding the current
    /// page, and the buttons.
    root: RefCell<Option<Retained<NSView>>>,
    slot: RefCell<Option<Retained<NSView>>>,
    pages: RefCell<Vec<Retained<NSView>>>,
    picker: RefCell<Option<Retained<NSSegmentedControl>>>,
    /// `--controls`: keep the panel up regardless of ep's full-screen state,
    /// until it is closed.
    pinned: Cell<bool>,
    chan_rows: RefCell<Option<ChanRows>>,
    /// Refreshes so far, for pacing the readouts.
    refreshes: Cell<u64>,
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
        /// panels; a pinned panel unpins, so it stays closed.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &AnyObject) -> bool {
            self.ivars().pinned.set(false);
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
            root: RefCell::new(None),
            slot: RefCell::new(None),
            pages: RefCell::new(Vec::new()),
            picker: RefCell::new(None),
            pinned: Cell::new(pinned),
            chan_rows: RefCell::new(None),
            refreshes: Cell::new(0),
        });
        // SAFETY: NSObject's initialiser, once, on a fresh allocation.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        let pages = vec![this.shape_page(mtm), this.colour_page(mtm)];
        let picker = this.page_picker(mtm, &["Shape", "Colour"]);
        let bar = this.button_bar(mtm);
        // Plain constraints rather than a stack view, so the window fits the
        // page exactly: a stack view doesn't insist on containing its views
        // across its width.
        let root = NSView::new(mtm);
        let slot = NSView::new(mtm);
        let picker_view = into_view(&*picker);
        for v in [&picker_view, &slot, &bar] {
            v.setTranslatesAutoresizingMaskIntoConstraints(false);
            root.addSubview(v);
        }
        activate(&[
            picker_view
                .topAnchor()
                .constraintEqualToAnchor_constant(&root.topAnchor(), 14.0),
            slot.topAnchor()
                .constraintEqualToAnchor_constant(&picker_view.bottomAnchor(), MARGIN),
            bar.topAnchor()
                .constraintEqualToAnchor_constant(&slot.bottomAnchor(), MARGIN),
            root.bottomAnchor()
                .constraintEqualToAnchor_constant(&bar.bottomAnchor(), MARGIN),
            picker_view
                .centerXAnchor()
                .constraintEqualToAnchor(&root.centerXAnchor()),
            slot.leadingAnchor()
                .constraintEqualToAnchor_constant(&root.leadingAnchor(), MARGIN),
            root.trailingAnchor()
                .constraintEqualToAnchor_constant(&slot.trailingAnchor(), MARGIN),
            bar.leadingAnchor()
                .constraintEqualToAnchor(&slot.leadingAnchor()),
            bar.trailingAnchor()
                .constraintEqualToAnchor(&slot.trailingAnchor()),
        ]);
        for page in &pages {
            page.setTranslatesAutoresizingMaskIntoConstraints(false);
        }

        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::UtilityWindow,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: `Retained` owns the panel, so AppKit must not release it too.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setTitle(&ns("Controls"));
        panel.setContentView(Some(&root));
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setLevel(level + 1);
        panel.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        *this.ivars().window.borrow_mut() = Some(panel.clone());
        *this.ivars().root.borrow_mut() = Some(root);
        *this.ivars().slot.borrow_mut() = Some(slot);
        *this.ivars().pages.borrow_mut() = pages;
        *this.ivars().picker.borrow_mut() = Some(picker);
        this.show_page(0, false);
        panel.center();

        // The run loop retains the timer, which runs until the app quits.
        // SAFETY: `refresh:` is defined on this class and takes the timer.
        let _ = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                1.0 / REFRESH_HZ,
                &this,
                sel!(refresh:),
                None,
                true,
            )
        };
        this.refresh();
        this
    }

    /// Draws the panel, title bar included, showing each page into
    /// `<path>-<page>.png`, and in dark mode into `<path>-<page>-dark.png`
    /// (for checking the layout without a screen).
    pub(crate) fn snapshot(&self, path: &str) -> Result<(), String> {
        let window = self.ivars().window.borrow().clone().ok_or("no panel")?;
        let root = self.ivars().root.borrow().clone().ok_or("no panel")?;
        // The window's frame view draws its background and title bar too.
        // SAFETY: the window (which `window` retains) owns its frame view
        // for as long as it has this content view.
        let frame = unsafe { root.superview() }.ok_or("panel not in a window")?;
        // SAFETY: a constant AppKit defines.
        let dark = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua });
        self.refresh_values();
        for page in 0..self.ivars().pages.borrow().len() {
            self.show_page(page, false);
            for (suffix, appearance) in [("", None), ("-dark", dark.as_deref())] {
                window.setAppearance(appearance);
                frame.layoutSubtreeIfNeeded();
                let rect = frame.bounds();
                let rep = frame
                    .bitmapImageRepForCachingDisplayInRect(rect)
                    .ok_or("no bitmap")?;
                frame.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
                // SAFETY: an empty properties dictionary is valid for every file type.
                let png = unsafe {
                    rep.representationUsingType_properties(
                        objc2_app_kit::NSBitmapImageFileType::PNG,
                        &objc2_foundation::NSDictionary::new(),
                    )
                }
                .ok_or("no png")?;
                let file = format!("{path}-{page}{suffix}.png");
                std::fs::write(&file, png.to_vec()).map_err(|e| e.to_string())?;
            }
        }
        window.setAppearance(None);
        self.show_page(0, false);
        Ok(())
    }

    /// Shows page `index` and fits the window to it, keeping its top edge
    /// where it is.
    fn show_page(&self, index: usize, animate: bool) {
        if let (Some(slot), Some(page)) = (
            self.ivars().slot.borrow().as_ref(),
            self.ivars().pages.borrow().get(index),
        ) {
            for old in slot.subviews().iter() {
                old.removeFromSuperview();
            }
            slot.addSubview(page);
            pin_edges(page, slot);
        }
        if let Some(picker) = self.ivars().picker.borrow().as_ref() {
            picker.setSelectedSegment(index as isize);
        }
        let (Some(window), Some(root)) = (
            self.ivars().window.borrow().clone(),
            self.ivars().root.borrow().clone(),
        ) else {
            return;
        };
        let size = root.fittingSize();
        let old = window.frame();
        let mut frame = window.frameRectForContentRect(NSRect::new(old.origin, size));
        frame.origin.y = old.origin.y + old.size.height - frame.size.height;
        window.setFrame_display_animate(frame, true, animate);
    }

    // ------------------------------------------------------------- layout

    fn shape_page(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let position = hstack(
            mtm,
            &[
                name(mtm, "X"),
                self.slider(mtm, -0.5, 0.5, Bind::Puck(0), 160.0),
                name(mtm, "Y"),
                self.slider(mtm, -0.5, 0.5, Bind::Puck(1), 160.0),
                self.button(mtm, "Centre", Bind::PuckCentre),
            ],
        );
        page(
            mtm,
            &[
                section(mtm, "Tiles", None, self.toggle_grid(mtm, &TILES)),
                section(mtm, "Effects", None, self.toggle_grid(mtm, &EFFECTS)),
                section(mtm, "Strand", None, self.strand_grid(mtm)),
                section(mtm, "Position", None, position),
                section(mtm, "Playback", None, self.toggle_grid(mtm, &PLAYBACK)),
            ],
        )
    }

    fn colour_page(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let backdrop = BACKDROP
            .iter()
            .map(|&(id, text)| {
                let (min, max) = self.range(id);
                vec![
                    name(mtm, text),
                    self.slider(mtm, min, max, Bind::Value(id), 200.0),
                    self.symbol_button(
                        mtm,
                        "arrow.counterclockwise",
                        "Reset",
                        Bind::Press(id + ids::DIAL_RESET),
                    ),
                ]
            })
            .collect();
        let backdrop = into_view(&*grid(mtm, backdrop));

        let mode = NSSegmentedControl::new(mtm);
        mode.setSegmentCount(2);
        mode.setLabel_forSegment(&ns("Hue"), 0);
        mode.setLabel_forSegment(&ns("Lightness"), 1);
        mode.setControlSize(NSControlSize::Small);
        let mode = self.add(&*mode, Bind::ChanMode);

        let (min, max) = self.range(ids::CHAN_RATE);
        let cycling = hstack(
            mtm,
            &[
                name(mtm, "Channel rate"),
                self.slider(mtm, min, max, Bind::Value(ids::CHAN_RATE), 300.0),
            ],
        );
        page(
            mtm,
            &[
                section(mtm, "Outline and background", None, backdrop),
                section(mtm, "Channels", Some(mode), self.channel_grid(mtm)),
                section(mtm, "Cycling", None, cycling),
            ],
        )
    }

    /// Checkboxes three to a row, each followed by its key in grey.
    fn toggle_grid(&self, mtm: MainThreadMarker, toggles: &[(u16, &str)]) -> Retained<NSView> {
        let rows = toggles
            .chunks(3)
            .map(|chunk| {
                let mut row = Vec::new();
                for &(id, text) in chunk {
                    row.push(self.checkbox(mtm, text, Bind::Press(id)));
                    row.push(caption(
                        mtm,
                        &self.key(id).map(String::from).unwrap_or_default(),
                    ));
                }
                while row.len() < 6 {
                    row.push(empty(mtm));
                }
                row
            })
            .collect();
        let g = grid(mtm, rows);
        g.setColumnSpacing(6.0);
        // Room between one toggle's key and the next toggle.
        for column in [1, 3] {
            g.columnAtIndex(column).setTrailingPadding(18.0);
        }
        into_view(&*g)
    }

    /// The ep sliders: value with its reading, the drift window, the rate,
    /// bounce and reset.
    fn strand_grid(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let mut rows = vec![
            ["", "Value", "", "Low", "High", "Rate", "", ""]
                .map(|s| caption(mtm, s))
                .to_vec(),
        ];
        for (id, text) in SLIDERS {
            let [value, low, high, rate] = self.ep_slider(mtm, id, 180.0, 70.0);
            rows.push(vec![
                name(mtm, text),
                value,
                self.readout(mtm, Bind::Readout(id)),
                low,
                high,
                rate,
                self.bounce_button(mtm, id),
                self.reset_button(mtm, id),
            ]);
        }
        into_view(&*grid(mtm, rows))
    }

    /// One column per colour channel.
    fn channel_grid(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let channels = 0..5;
        let header = std::iter::once(empty(mtm))
            .chain(
                channels
                    .clone()
                    .map(|i| caption(mtm, &format!("Channel {}", i + 1))),
            )
            .collect();
        let row = |text: &str, make: &dyn Fn(usize) -> Retained<NSView>| -> Vec<Retained<NSView>> {
            std::iter::once(name(mtm, text))
                .chain(channels.clone().map(make))
                .collect()
        };
        let mut rows: Vec<Vec<Retained<NSView>>> = vec![
            header,
            row("In use", &|i| {
                self.radio(mtm, Bind::Press(ids::active_button(i)))
            }),
            row("Colour", &|i| self.swatch(mtm, i)),
            row("Opacity", &|i| {
                self.slider(mtm, 0.0, 1.0, Bind::Value(ids::alpha_dial(i)), 90.0)
            }),
        ];
        let mut hue_rows = Vec::new();
        let mut light_rows = Vec::new();
        for (set, base) in [
            (&mut hue_rows, ids::hue_slider as fn(usize) -> u16),
            (&mut light_rows, ids::light_slider),
        ] {
            let sliders: Vec<_> = channels
                .clone()
                .map(|i| self.ep_slider(mtm, base(i), 90.0, 90.0))
                .collect();
            for (k, text) in ["Value", "Low", "High", "Rate"].into_iter().enumerate() {
                set.push(rows.len() as isize);
                rows.push(
                    std::iter::once(name(mtm, text))
                        .chain(sliders.iter().map(|s| s[k].clone()))
                        .collect(),
                );
            }
            set.push(rows.len() as isize);
            rows.push(row("", &|i| {
                into_view(&*hstack(
                    mtm,
                    &[
                        self.bounce_button(mtm, base(i)),
                        self.reset_button(mtm, base(i)),
                    ],
                ))
            }));
        }
        rows.push(row("Dwell", &|i| {
            self.slider(mtm, 0.0, 1.0, Bind::Value(ids::dwell(i)), 90.0)
        }));
        let g = grid(mtm, rows);
        g.setColumnSpacing(12.0);
        *self.ivars().chan_rows.borrow_mut() = Some(ChanRows {
            grid: g.clone(),
            hue: hue_rows,
            light: light_rows,
            hue_mode: Cell::new(None),
        });
        into_view(&*g)
    }

    fn page_picker(&self, mtm: MainThreadMarker, labels: &[&str]) -> Retained<NSSegmentedControl> {
        let picker = NSSegmentedControl::new(mtm);
        picker.setContentHuggingPriority_forOrientation(
            1000.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        picker.setSegmentCount(labels.len() as isize);
        for (i, text) in labels.iter().enumerate() {
            picker.setLabel_forSegment(&ns(text), i as isize);
        }
        self.add(&*picker, Bind::Page);
        picker
    }

    /// The buttons that act on the whole show, at the bottom right.
    fn button_bar(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let bar = stack(mtm, &[], NSUserInterfaceLayoutOrientation::Horizontal);
        for (text, id) in [("Reset All", ids::RESET), ("Quit", ids::EXIT)] {
            bar.addView_inGravity(
                &self.button(mtm, text, Bind::Press(id)),
                NSStackViewGravity::Trailing,
            );
        }
        into_view(&*bar)
    }

    // ----------------------------------------------------------- controls

    /// The key equivalent ep gives actuator `id`, if any.
    fn key(&self, id: u16) -> Option<char> {
        let show = self.ivars().show.borrow();
        show.sim()
            .actuator(id)
            .and_then(|a| a.key)
            .filter(|k| !k.is_control())
    }

    fn range(&self, id: u16) -> (f64, f64) {
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
    fn add(&self, control: &(impl AsRef<NSView> + ?Sized), bind: Bind) -> Retained<NSView> {
        self.add_with(control, bind, false, 0.0)
    }

    /// `add` for a symbol toggle (`tinted`) or a slider (`tolerance`); see
    /// `Control`.
    fn add_with(
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
        self.add_with(&*s, bind, false, KNOB_STEP * (max - min) / width)
    }

    fn checkbox(&self, mtm: MainThreadMarker, title: &str, bind: Bind) -> Retained<NSView> {
        // SAFETY: no target or action; `register` sets them.
        let b = unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), None, None, mtm) };
        self.add(&*b, bind)
    }

    fn radio(&self, mtm: MainThreadMarker, bind: Bind) -> Retained<NSView> {
        // SAFETY: no target or action; `register` sets them.
        let b = unsafe { NSButton::radioButtonWithTitle_target_action(&ns(""), None, None, mtm) };
        self.add(&*b, bind)
    }

    fn button(&self, mtm: MainThreadMarker, title: &str, bind: Bind) -> Retained<NSView> {
        // SAFETY: no target or action; `register` sets them.
        let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
        self.add(&*b, bind)
    }

    /// A borderless SF Symbols button, tinted with the accent colour while
    /// its actuator is on (secondary grey otherwise).
    fn symbol_button(
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
        // SAFETY: no target or action; `register` sets them.
        let b = unsafe { NSButton::buttonWithImage_target_action(&image, None, None, mtm) };
        b.setBordered(false);
        b.setToolTip(Some(&ns(tip)));
        self.add_with(&*b, bind, true, 0.0)
    }

    /// The value, low, high and rate sliders of the ep slider whose main
    /// actuator is `id`. The bounds take the value's range.
    fn ep_slider(
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
    fn bounce_button(&self, mtm: MainThreadMarker, id: u16) -> Retained<NSView> {
        self.symbol_button(
            mtm,
            "arrow.left.arrow.right",
            "Bounce at the ends of the range (off: wrap round)",
            Bind::Press(id + ids::SLIDER_BOUNCE),
        )
    }

    /// The "R" of the ep slider whose main actuator is `id`.
    fn reset_button(&self, mtm: MainThreadMarker, id: u16) -> Retained<NSView> {
        self.symbol_button(
            mtm,
            "arrow.counterclockwise",
            "Reset range, rate and value",
            Bind::Press(id + ids::SLIDER_RESET),
        )
    }

    fn readout(&self, mtm: MainThreadMarker, bind: Bind) -> Retained<NSView> {
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

    fn swatch(&self, mtm: MainThreadMarker, channel: usize) -> Retained<NSView> {
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

    // ------------------------------------------------------------ events

    fn on_change(&self, index: usize) {
        // Take what's needed and let go of the borrow: acting on it may run
        // the run loop, and with it the show and this panel's refresh.
        let (bind, view) = {
            let controls = self.ivars().controls.borrow();
            let Some(c) = controls.get(index) else { return };
            c.edited.set(Some(Instant::now()));
            // The user moved it: show the simulation's value again afterwards
            // even if that hasn't changed.
            c.shown.set(None);
            if matches!(c.bind, Bind::Press(_)) {
                // A press can change controls nobody touched: AppKit turns
                // off the other radios of a group itself, and a reset moves
                // many sliders. Show them all afresh.
                for other in controls.iter() {
                    other.shown.set(None);
                }
            }
            (c.bind, c.view.clone())
        };
        if matches!(bind, Bind::Page) {
            // The panel's own state. The switch animates the window's
            // resize, so it runs with no borrows held.
            if let Some(s) = view.downcast_ref::<NSSegmentedControl>() {
                self.show_page(s.selectedSegment().max(0) as usize, true);
            }
            return;
        }
        let mut show = self.ivars().show.borrow_mut();
        let sim = show.sim_mut();
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
            Bind::Page | Bind::Readout(_) | Bind::Swatch(_) => {}
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
        drop(window_ref);
        drop(show);
        self.refresh_values();
    }

    /// Sets every control from the simulation (skipping ones just edited).
    fn refresh_values(&self) {
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
            Bind::PuckCentre | Bind::Page => [0.0; 2],
        }
    }
}

/// Sets control `c` to show `state`, as computed by `state_of`.
fn show_control(c: &Control, state: [f32; 2]) {
    let [a, b] = state;
    match c.bind {
        Bind::Value(_) | Bind::Puck(_) => {
            if let (Some(s), false) = (c.view.downcast_ref::<NSControl>(), a.is_nan()) {
                s.setDoubleValue(f64::from(a));
            }
        }
        Bind::Press(_) => {
            if let Some(button) = c.view.downcast_ref::<NSButton>() {
                show_on(button, a != 0.0, c.tinted);
            }
        }
        Bind::Readout(_) => {
            if let (Some(t), false) = (c.view.downcast_ref::<NSTextField>(), a.is_nan()) {
                t.setStringValue(&NSString::from_str(&format!("{a:.3}")));
            }
        }
        Bind::Swatch(_) => {
            if let Some(swatch) = c.view.downcast_ref::<NSBox>() {
                let hue = if a.is_nan() { 0.0 } else { a };
                let light = if b.is_nan() { 0.5 } else { b };
                swatch.setFillColor(&swatch_colour(hue, light));
            }
        }
        Bind::ChanMode => {
            if let Some(s) = c.view.downcast_ref::<NSSegmentedControl>() {
                // Hue mode shows segment 0, lightness segment 1.
                s.setSelectedSegment(isize::from(a == 0.0));
            }
        }
        Bind::PuckCentre | Bind::Page => {}
    }
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

// ----------------------------------------------------------------- helpers

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Any view, as an `NSView`.
fn into_view(v: &(impl AsRef<NSView> + ?Sized)) -> Retained<NSView> {
    v.as_ref().retain()
}

fn text(mtm: MainThreadMarker, s: &str) -> Retained<NSTextField> {
    NSTextField::labelWithString(&ns(s), mtm)
}

/// A row or field name.
fn name(mtm: MainThreadMarker, s: &str) -> Retained<NSView> {
    into_view(&*text(mtm, s))
}

/// Small secondary text: column headings and key hints.
fn caption(mtm: MainThreadMarker, s: &str) -> Retained<NSView> {
    let t = text(mtm, s);
    t.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    t.setTextColor(Some(&NSColor::secondaryLabelColor()));
    into_view(&*t)
}

fn empty(mtm: MainThreadMarker) -> Retained<NSView> {
    NSGridCell::emptyContentView(mtm)
}

fn grid(mtm: MainThreadMarker, rows: Vec<Vec<Retained<NSView>>>) -> Retained<NSGridView> {
    let rows: Vec<Retained<NSArray<NSView>>> = rows
        .into_iter()
        .map(|r| NSArray::from_retained_slice(&r))
        .collect();
    let g = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&rows), mtm);
    g.setRowSpacing(6.0);
    g.setColumnSpacing(10.0);
    // Labels level with the controls beside them.
    g.setRowAlignment(NSGridRowAlignment::None);
    g.setYPlacement(NSGridCellPlacement::Center);
    // Keep rows at their natural height when the section is taller.
    g.setContentHuggingPriority_forOrientation(999.0, NSLayoutConstraintOrientation::Vertical);
    g
}

fn stack(
    mtm: MainThreadMarker,
    views: &[Retained<NSView>],
    o: NSUserInterfaceLayoutOrientation,
) -> Retained<NSStackView> {
    let s = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(views), mtm);
    s.setOrientation(o);
    s
}

fn hstack(mtm: MainThreadMarker, views: &[Retained<NSView>]) -> Retained<NSView> {
    let s = stack(mtm, views, NSUserInterfaceLayoutOrientation::Horizontal);
    s.setSpacing(10.0);
    into_view(&*s)
}

/// Makes `child` as wide as `parent`.
fn fill_width(child: &NSView, parent: &NSView) {
    child
        .widthAnchor()
        .constraintEqualToAnchor(&parent.widthAnchor())
        .setActive(true);
}

/// A page: its sections one under another, all the same width.
fn page(mtm: MainThreadMarker, sections: &[Retained<NSView>]) -> Retained<NSView> {
    let s = stack(mtm, sections, NSUserInterfaceLayoutOrientation::Vertical);
    s.setAlignment(NSLayoutAttribute::Leading);
    s.setSpacing(18.0);
    for section in sections {
        fill_width(section, &s);
    }
    into_view(&*s)
}

/// A section in the System Settings style: a heading, with an optional
/// control at its trailing end, over a rounded card holding `content`.
fn section(
    mtm: MainThreadMarker,
    title: &str,
    accessory: Option<Retained<NSView>>,
    content: Retained<NSView>,
) -> Retained<NSView> {
    let label = text(mtm, title);
    label.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, 0.3)));
    let heading = stack(
        mtm,
        &[into_view(&*label)],
        NSUserInterfaceLayoutOrientation::Horizontal,
    );
    if let Some(accessory) = accessory {
        heading.addView_inGravity(&accessory, NSStackViewGravity::Trailing);
    }
    let card = card(mtm, content);
    let heading = into_view(&*heading);
    let s = stack(
        mtm,
        &[heading.clone(), card.clone()],
        NSUserInterfaceLayoutOrientation::Vertical,
    );
    s.setAlignment(NSLayoutAttribute::Leading);
    s.setSpacing(6.0);
    fill_width(&heading, &s);
    fill_width(&card, &s);
    into_view(&*s)
}

/// `content` on AppKit's group box background (which follows light and dark
/// mode), at its natural width at the leading edge however wide the card is.
/// Without the high hugging, the content's default hugging ties with the
/// stack view's pull towards its edges (both at low priority), and Auto
/// Layout settles the tie differently from run to run, spreading a grid's
/// columns out.
fn card(mtm: MainThreadMarker, content: Retained<NSView>) -> Retained<NSView> {
    content
        .setContentHuggingPriority_forOrientation(999.0, NSLayoutConstraintOrientation::Horizontal);
    let s = stack(
        mtm,
        &[content],
        NSUserInterfaceLayoutOrientation::Horizontal,
    );
    s.setEdgeInsets(NSEdgeInsets {
        top: 12.0,
        left: 14.0,
        bottom: 12.0,
        right: 14.0,
    });
    let background = NSBox::new(mtm);
    background.setBoxType(NSBoxType::Primary);
    background.setTitlePosition(NSTitlePosition::NoTitle);
    background.setTranslatesAutoresizingMaskIntoConstraints(false);
    s.addSubview_positioned_relativeTo(&background, NSWindowOrderingMode::Below, None);
    pin_edges(&background, &s);
    into_view(&*s)
}

/// Activates `constraints` together.
fn activate(constraints: &[Retained<NSLayoutConstraint>]) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(constraints));
}

/// Pins `child`'s edges to `parent`'s.
fn pin_edges(child: &NSView, parent: &NSView) {
    activate(&[
        child
            .leadingAnchor()
            .constraintEqualToAnchor(&parent.leadingAnchor()),
        child
            .trailingAnchor()
            .constraintEqualToAnchor(&parent.trailingAnchor()),
        child
            .topAnchor()
            .constraintEqualToAnchor(&parent.topAnchor()),
        child
            .bottomAnchor()
            .constraintEqualToAnchor(&parent.bottomAnchor()),
    ]);
}
