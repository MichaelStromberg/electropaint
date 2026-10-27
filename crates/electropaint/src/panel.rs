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

mod controls;
mod layout;
mod pages;
mod sync;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use electropaint_core::ids;
use electropaint_metal::show::Show;
use electropaint_metal::surface::Style;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSBackingStoreType,
    NSControl, NSPanel, NSSegmentedControl, NSView, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSTimer};

use layout::{activate, into_view, ns, pin_edges};
use sync::{ChanRows, REFRESH_HZ};

/// Space around the window's content and between its parts.
const MARGIN: f64 = 20.0;

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
    /// How the show is drawn, classic or remastered (not the simulation's).
    Style,
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

/// The window and the views that frame the pages, made once in `new`.
struct Chrome {
    window: Retained<NSPanel>,
    /// The window's content: the page picker, the slot holding the current
    /// page, and the buttons.
    root: Retained<NSView>,
    slot: Retained<NSView>,
    pages: Vec<Retained<NSView>>,
    picker: Retained<NSSegmentedControl>,
}

pub(crate) struct PanelIvars {
    show: Rc<RefCell<Show>>,
    /// The style the show windows draw in.
    style: Rc<Cell<Style>>,
    controls: RefCell<Vec<Control>>,
    puck: Cell<[f32; 2]>,
    chrome: OnceCell<Chrome>,
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
    /// the show, which the panel must float above; `style` is the drawing
    /// style the show windows follow.
    pub(crate) fn new(
        mtm: MainThreadMarker,
        show: Rc<RefCell<Show>>,
        style: Rc<Cell<Style>>,
        pinned: bool,
        level: isize,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PanelIvars {
            show,
            style,
            controls: RefCell::new(Vec::new()),
            puck: Cell::new([0.0; 2]),
            chrome: OnceCell::new(),
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
        let chrome = Chrome {
            window: panel.clone(),
            root,
            slot,
            pages,
            picker,
        };
        if this.ivars().chrome.set(chrome).is_err() {
            unreachable!("the chrome is made once, here");
        }
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
        let Chrome {
            window,
            root,
            pages,
            ..
        } = self.ivars().chrome.get().ok_or("no panel")?;
        // The window's frame view draws its background and title bar too.
        // SAFETY: the window (which `window` retains) owns its frame view
        // for as long as it has this content view.
        let frame = unsafe { root.superview() }.ok_or("panel not in a window")?;
        // SAFETY: a constant AppKit defines.
        let dark = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua });
        self.refresh_values();
        for page in 0..pages.len() {
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
        let Some(chrome) = self.ivars().chrome.get() else {
            return;
        };
        let Chrome {
            window,
            root,
            slot,
            pages,
            picker,
        } = chrome;
        if let Some(page) = pages.get(index) {
            for old in slot.subviews().iter() {
                old.removeFromSuperview();
            }
            slot.addSubview(page);
            pin_edges(page, slot);
        }
        picker.setSelectedSegment(index as isize);
        let size = root.fittingSize();
        let old = window.frame();
        let mut frame = window.frameRectForContentRect(NSRect::new(old.origin, size));
        frame.origin.y = old.origin.y + old.size.height - frame.size.height;
        window.setFrame_display_animate(frame, true, animate);
    }
}
