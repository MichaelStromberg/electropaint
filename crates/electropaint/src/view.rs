//! The show windows: a view per window running its strands' shows from the
//! display link, and the window around it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ep_metal::show::Show;
use ep_metal::surface::{Strands, Style, Surface, main_strand};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSEvent, NSEventModifierFlags, NSScreen, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString};
use objc2_quartz_core::CADisplayLink;

use crate::quit;

struct ViewState {
    /// The style to draw in, which the control panel can change.
    style: Rc<Cell<Style>>,
    /// One per strand, left to right.
    shows: Vec<Rc<RefCell<Show>>>,
    /// The strand that keys and clicks go to (the one the panel drives).
    main: usize,
    surface: Surface,
    /// Screensaver-style: any key or click quits. Otherwise input goes to ep.
    quit_on_input: bool,
}

/// Named so rust-analyzer can read it inside `define_class!`, which it
/// cannot do for a generic type written in place.
type ViewIvars = RefCell<Option<ViewState>>;

define_class!(
    /// A view hosting a `CAMetalLayer` that runs shows side by side, driven
    /// by its display's own display link (so at that display's refresh rate).
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "EPShowView"]
    #[ivars = ViewIvars]
    struct ShowView;

    impl ShowView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.key(event);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            self.click();
        }

        #[unsafe(method(step:))]
        fn step(&self, link: &CADisplayLink) {
            // When this refresh reaches the screen, which is what the
            // in-between pictures should be timed to.
            self.advance(link.targetTimestamp());
        }
    }
);

define_class!(
    /// Borderless windows can't become key by default; ours must, to get keys.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "EPShowWindow"]
    struct ShowWindow;

    impl ShowWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// Closing the show (only `--windowed` has a close button) quits:
        /// with no window and no menu bar there would be no way left to.
        #[unsafe(method(close))]
        fn close(&self) {
            // SAFETY: NSWindow's own `close`, which overrides must call.
            let _: () = unsafe { msg_send![super(self), close] };
            quit(MainThreadMarker::from(self));
        }
    }
);

impl ShowView {
    fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        style: Rc<Cell<Style>>,
        shows: Vec<Rc<RefCell<Show>>>,
        quit_on_input: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(None));
        // SAFETY: NSView's designated initialiser, once, on a fresh allocation.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };

        // Remastered falls back to classic, as a switch from the panel does.
        let surface = Surface::new(style.get())
            .or_else(|e| {
                eprintln!("electropaint: {e}");
                style.set(Style::Classic);
                Surface::new(Style::Classic)
            })
            .unwrap_or_else(|e| {
                eprintln!("electropaint: {e}");
                std::process::exit(1)
            });
        // Layer-hosting view: set the layer before asking for one.
        view.setLayer(Some(surface.layer()));
        view.setWantsLayer(true);

        // SAFETY: `step:` is defined on this class and takes the `CADisplayLink`.
        let link = unsafe { view.displayLinkWithTarget_selector(&view as &AnyObject, sel!(step:)) };
        // In the common modes, so the show keeps running while a panel
        // slider is dragged or the window resized (event tracking mode).
        // SAFETY: the main run loop, from the main thread (`MainThreadOnly`).
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        // The run loop retains the link, which runs until the app quits.

        *view.ivars().borrow_mut() = Some(ViewState {
            style,
            main: main_strand(shows.len()),
            shows,
            surface,
            quit_on_input,
        });
        view
    }

    /// Renders what the shows have for a refresh at `time` seconds.
    fn advance(&self, time: f64) {
        let mut state = self.ivars().borrow_mut();
        let Some(st) = state.as_mut() else { return };

        // Keep the drawable at the view's size in device pixels.
        let window = self.window();
        let scale = window.as_ref().map_or(1.0, |w| w.backingScaleFactor());
        let headroom = window
            .and_then(|w| w.screen())
            .map_or(1.0, |s| s.maximumExtendedDynamicRangeColorComponentValue());
        st.surface.fit(self.bounds().size, scale, headroom);
        if let Err(e) = st.surface.set_style(st.style.get()) {
            eprintln!("electropaint: {e}");
            st.style.set(Style::Classic);
        }

        let exited = {
            let mut shows: Vec<_> = st.shows.iter().map(|s| s.borrow_mut()).collect();
            st.surface.render(shows.iter_mut().map(|s| &mut **s), time);
            shows.iter().any(|s| s.sim().exited())
        };
        drop(state);
        if exited {
            quit(MainThreadMarker::from(self));
        }
    }

    /// Fullscreen: any key quits. Windowed: keys are ep's own shortcuts,
    /// and Esc (or ep's exit) quits. Command-key chords aren't ep's (the
    /// menu has taken the ones it knows, such as ⌘Q).
    fn key(&self, event: &NSEvent) {
        let chars = event
            .charactersIgnoringModifiers()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let command = event
            .modifierFlags()
            .contains(NSEventModifierFlags::Command);
        let quitting = {
            let mut state = self.ivars().borrow_mut();
            let Some(st) = state.as_mut() else { return };
            if st.quit_on_input || chars == "\u{1b}" {
                true
            } else if command {
                false
            } else {
                let mut show = st.shows[st.main].borrow_mut();
                for c in chars.chars() {
                    show.sim_mut().key(c);
                }
                false
            }
        };
        if quitting {
            quit(MainThreadMarker::from(self));
        }
    }

    /// Fullscreen: quit. Interactive: ep's mouse actuator, which toggles
    /// its full-screen state (and with it the panels).
    fn click(&self) {
        let quitting = {
            let state = self.ivars().borrow();
            let Some(st) = state.as_ref() else { return };
            if !st.quit_on_input {
                st.shows[st.main]
                    .borrow_mut()
                    .sim_mut()
                    .press(ep_core::ids::MOUSE);
            }
            st.quit_on_input
        };
        if quitting {
            quit(MainThreadMarker::from(self));
        }
    }
}

/// Opens a window running `strands` shows side by side, each made by
/// `new_show`: borderless and covering `screen`, above the menu bar and Dock
/// like a screen saver, or with no screen the `--windowed` window.
/// `quit_on_input` makes any key or click quit. Also returns the show that
/// takes the window's input, the middle one.
pub(crate) fn open_window(
    mtm: MainThreadMarker,
    screen: Option<&NSScreen>,
    strands: Strands,
    new_show: &mut dyn FnMut() -> Rc<RefCell<Show>>,
    style: &Rc<Cell<Style>>,
    quit_on_input: bool,
) -> (Retained<NSWindow>, Rc<RefCell<Show>>) {
    let (rect, mask) = match screen {
        Some(screen) => (screen.frame(), NSWindowStyleMask::Borderless),
        // The original's window: 1280x1024 aspect, at a comfortable size.
        None => (
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 800.0)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
        ),
    };
    // SAFETY: NSWindow's designated initialiser, with its argument types, on a
    // fresh allocation.
    let window: Retained<ShowWindow> = unsafe {
        msg_send![
            ShowWindow::alloc(mtm),
            initWithContentRect: rect,
            styleMask: mask,
            backing: NSBackingStoreType::Buffered,
            defer: false,
            screen: screen,
        ]
    };
    let window: Retained<NSWindow> = Retained::into_super(window);
    // SAFETY: `Retained` owns the window, so AppKit must not release it too.
    unsafe { window.setReleasedWhenClosed(false) };
    let content = NSRect::new(NSPoint::new(0.0, 0.0), rect.size);
    let count = strands.count(rect.size.width, rect.size.height);
    let shows: Vec<_> = (0..count).map(|_| new_show()).collect();
    let main = shows[main_strand(count)].clone();
    let view = ShowView::new(mtm, content, style.clone(), shows, quit_on_input);
    window.setContentView(Some(&view));
    if screen.is_some() {
        window.setLevel(SHOW_LEVEL);
        window.setFrame_display(rect, true);
    } else {
        window.setTitle(&NSString::from_str("electropaint"));
        window.center();
    }
    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&view));
    (window, main)
}

/// Window level of fullscreen shows: above the menu bar and Dock.
pub(crate) const SHOW_LEVEL: isize = 1000;
