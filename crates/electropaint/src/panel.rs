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
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSApplication,
    NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSControl, NSControlSize,
    NSControlStateValueOff, NSControlStateValueOn, NSFont, NSGridCell, NSGridCellPlacement,
    NSGridRowAlignment, NSGridView, NSImage, NSLayoutAttribute, NSLayoutConstraintOrientation,
    NSPanel, NSSegmentedControl, NSSlider, NSStackView, NSStackViewGravity, NSTextAlignment,
    NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView, NSWindowDelegate,
    NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
};

/// After the user moves a control, the panel leaves it alone this long
/// before following the simulation again, so a drag isn't fought.
const EDIT_GRACE: Duration = Duration::from_millis(400);
const REFRESH_HZ: f64 = 20.0;
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
}

/// The channel grid and its rows for the hue sliders and for the
/// lightness sliders (shown one set at a time, like ep's two overlapping
/// slider sets).
struct ChanRows {
    grid: Retained<NSGridView>,
    hue: Vec<isize>,
    light: Vec<isize>,
}

impl ChanRows {
    fn show(&self, hue_mode: bool) {
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
    /// `--controls`: keep the panel up regardless of ep's full-screen state.
    pinned: bool,
    chan_rows: RefCell<Option<ChanRows>>,
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
            root: RefCell::new(None),
            slot: RefCell::new(None),
            pages: RefCell::new(Vec::new()),
            picker: RefCell::new(None),
            pinned,
            chan_rows: RefCell::new(None),
            timer: RefCell::new(None),
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
        let picker_view = into_view(picker.clone());
        for v in [&picker_view, &slot, &bar] {
            v.setTranslatesAutoresizingMaskIntoConstraints(false);
            root.addSubview(v);
        }
        for c in [
            picker_view
                .topAnchor()
                .constraintEqualToAnchor_constant(&root.topAnchor(), 14.0),
            slot.topAnchor()
                .constraintEqualToAnchor_constant(&picker_view.bottomAnchor(), MARGIN),
            bar.topAnchor()
                .constraintEqualToAnchor_constant(&slot.bottomAnchor(), MARGIN),
            root.bottomAnchor()
                .constraintEqualToAnchor_constant(&bar.bottomAnchor(), MARGIN),
        ] {
            c.setActive(true);
        }
        for c in [
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
        ] {
            c.setActive(true);
        }
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

        // SAFETY: `refresh:` is defined on this class and takes the timer.
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
            for (a, b) in [
                (page.leadingAnchor(), slot.leadingAnchor()),
                (page.trailingAnchor(), slot.trailingAnchor()),
            ] {
                a.constraintEqualToAnchor(&b).setActive(true);
            }
            for (a, b) in [
                (page.topAnchor(), slot.topAnchor()),
                (page.bottomAnchor(), slot.bottomAnchor()),
            ] {
                a.constraintEqualToAnchor(&b).setActive(true);
            }
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
                    self.symbol_button(mtm, "arrow.counterclockwise", "Reset", Bind::Press(id + 1)),
                ]
            })
            .collect();
        let backdrop = into_view(narrow(grid(mtm, backdrop)));

        let mode = NSSegmentedControl::new(mtm);
        mode.setSegmentCount(2);
        mode.setLabel_forSegment(&ns("Hue"), 0);
        mode.setLabel_forSegment(&ns("Lightness"), 1);
        mode.setControlSize(NSControlSize::Small);
        let mode: Retained<NSView> = Retained::into_super(Retained::into_super(mode));
        self.register(&mode, Bind::ChanMode);

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
        let g = narrow(grid(mtm, rows));
        g.setColumnSpacing(6.0);
        // Room between one toggle's key and the next toggle.
        for column in [1, 3] {
            g.columnAtIndex(column).setTrailingPadding(18.0);
        }
        into_view(g)
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
            let (min, max) = self.range(id);
            let (rmin, rmax) = self.range(id + ids::SLIDER_RATE);
            rows.push(vec![
                name(mtm, text),
                self.slider(mtm, min, max, Bind::Value(id), 180.0),
                self.readout(mtm, Bind::Readout(id)),
                self.slider(mtm, min, max, Bind::Value(id + ids::SLIDER_LOWER), 70.0),
                self.slider(mtm, min, max, Bind::Value(id + ids::SLIDER_UPPER), 70.0),
                self.slider(mtm, rmin, rmax, Bind::Value(id + ids::SLIDER_RATE), 70.0),
                self.bounce_button(mtm, id),
                self.reset_button(mtm, id),
            ]);
        }
        into_view(narrow(grid(mtm, rows)))
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
            for (text, offset) in [
                ("Value", None),
                ("Low", Some(ids::SLIDER_LOWER)),
                ("High", Some(ids::SLIDER_UPPER)),
                ("Rate", Some(ids::SLIDER_RATE)),
            ] {
                set.push(rows.len() as isize);
                rows.push(row(text, &|i| {
                    let id = base(i) + offset.unwrap_or(0);
                    let (min, max) = self.range(id);
                    self.slider(mtm, min, max, Bind::Value(id), 90.0)
                }));
            }
            set.push(rows.len() as isize);
            rows.push(row("", &|i| {
                into_view(hstack(
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
        let g = narrow(grid(mtm, rows));
        g.setColumnSpacing(12.0);
        *self.ivars().chan_rows.borrow_mut() = Some(ChanRows {
            grid: g.clone(),
            hue: hue_rows,
            light: light_rows,
        });
        into_view(g)
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
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(picker.clone()));
        self.register(&view, Bind::Page);
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
        into_view(bar)
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

    fn register(&self, view: &Retained<NSView>, bind: Bind) {
        self.register_control(view, bind, false);
    }

    fn register_control(&self, view: &Retained<NSView>, bind: Bind, tinted: bool) {
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

    fn checkbox(&self, mtm: MainThreadMarker, title: &str, bind: Bind) -> Retained<NSView> {
        // SAFETY: no target or action; `register` sets them.
        let b = unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), None, None, mtm) };
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(b));
        self.register(&view, bind);
        view
    }

    fn radio(&self, mtm: MainThreadMarker, bind: Bind) -> Retained<NSView> {
        // SAFETY: no target or action; `register` sets them.
        let b = unsafe { NSButton::radioButtonWithTitle_target_action(&ns(""), None, None, mtm) };
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(b));
        self.register(&view, bind);
        view
    }

    fn button(&self, mtm: MainThreadMarker, title: &str, bind: Bind) -> Retained<NSView> {
        // SAFETY: no target or action; `register` sets them.
        let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(b));
        self.register(&view, bind);
        view
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
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(b));
        self.register_control(&view, bind, true);
        view
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
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(t));
        self.register(&view, bind);
        view
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
                let lightness = !sim.controls().hue_mode;
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
            Bind::Page => {
                if let Some(s) = c.view.downcast_ref::<NSSegmentedControl>() {
                    self.show_page(s.selectedSegment().max(0) as usize, true);
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
            self.show_control(c, sim);
        }
        if let Some(rows) = self.ivars().chan_rows.borrow().as_ref() {
            rows.show(sim.controls().hue_mode);
        }
    }

    /// Sets one control from the simulation.
    fn show_control(&self, c: &Control, sim: &Sim) {
        match c.bind {
            Bind::Value(id) => {
                if let (Some(s), Some(v)) = (c.view.downcast_ref::<NSControl>(), sim.value(id)) {
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
                    if c.tinted {
                        let tint = if on {
                            NSColor::controlAccentColor()
                        } else {
                            NSColor::secondaryLabelColor()
                        };
                        b.setContentTintColor(Some(&tint));
                    }
                }
            }
            Bind::Readout(id) => {
                if let (Some(t), Some(v)) = (c.view.downcast_ref::<NSTextField>(), sim.value(id)) {
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
                    let lightness = !sim.controls().hue_mode;
                    s.setSelectedSegment(lightness as isize);
                }
            }
            Bind::PuckCentre | Bind::Page => {}
        }
    }
}

// ----------------------------------------------------------------- helpers

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Any view, as an `NSView`.
fn into_view<T>(v: Retained<T>) -> Retained<NSView>
where
    T: objc2::Message + AsRef<NSView>,
{
    // SAFETY: `AsRef<NSView>` is implemented exactly for NSView's
    // subclasses, so the object is an NSView.
    unsafe { Retained::cast_unchecked(v) }
}

fn text(mtm: MainThreadMarker, s: &str) -> Retained<NSTextField> {
    NSTextField::labelWithString(&ns(s), mtm)
}

/// A row or field name.
fn name(mtm: MainThreadMarker, s: &str) -> Retained<NSView> {
    into_view(text(mtm, s))
}

/// Small secondary text: column headings and key hints.
fn caption(mtm: MainThreadMarker, s: &str) -> Retained<NSView> {
    let t = text(mtm, s);
    t.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    t.setTextColor(Some(&NSColor::secondaryLabelColor()));
    into_view(t)
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

/// Keeps a grid's columns at their natural width in a wider section.
/// Otherwise the grid's default hugging ties with the stack view's pull
/// towards its edges (both at low priority), and Auto Layout settles the tie
/// differently from run to run, spreading the columns out.
fn narrow(g: Retained<NSGridView>) -> Retained<NSGridView> {
    g.setContentHuggingPriority_forOrientation(999.0, NSLayoutConstraintOrientation::Horizontal);
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
    into_view(s)
}

/// Makes `child` as wide as `parent` less `inset` on each side.
fn fill_width(child: &NSView, parent: &NSView, inset: f64) {
    child
        .widthAnchor()
        .constraintEqualToAnchor_constant(&parent.widthAnchor(), -2.0 * inset)
        .setActive(true);
}

/// A page: its sections one under another, all the same width.
fn page(mtm: MainThreadMarker, sections: &[Retained<NSView>]) -> Retained<NSView> {
    let s = stack(mtm, sections, NSUserInterfaceLayoutOrientation::Vertical);
    s.setAlignment(NSLayoutAttribute::Leading);
    s.setSpacing(18.0);
    for section in sections {
        fill_width(section, &s, 0.0);
    }
    into_view(s)
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
        &[into_view(label)],
        NSUserInterfaceLayoutOrientation::Horizontal,
    );
    if let Some(accessory) = accessory {
        heading.addView_inGravity(&accessory, NSStackViewGravity::Trailing);
    }
    let card = card(mtm, content);
    let heading = into_view(heading);
    let s = stack(
        mtm,
        &[heading.clone(), card.clone()],
        NSUserInterfaceLayoutOrientation::Vertical,
    );
    s.setAlignment(NSLayoutAttribute::Leading);
    s.setSpacing(6.0);
    fill_width(&heading, &s, 0.0);
    fill_width(&card, &s, 0.0);
    into_view(s)
}

/// `content` on AppKit's group box background (which follows light and dark
/// mode), at its natural width at the leading edge however wide the card is.
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
    for (a, b) in [
        (background.leadingAnchor(), s.leadingAnchor()),
        (background.trailingAnchor(), s.trailingAnchor()),
    ] {
        a.constraintEqualToAnchor(&b).setActive(true);
    }
    for (a, b) in [
        (background.topAnchor(), s.topAnchor()),
        (background.bottomAnchor(), s.bottomAnchor()),
    ] {
        a.constraintEqualToAnchor(&b).setActive(true);
    }
    into_view(s)
}
