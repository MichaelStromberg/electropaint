//! electropaint as a macOS screen saver.
//!
//! The bundle's principal class, `EPSaverView`, is a `ScreenSaverView` that
//! hosts a `CAMetalLayer` and runs one show per instance (one per display,
//! plus the small preview in System Settings).

use std::cell::RefCell;
use std::sync::atomic::{AtomicI64, Ordering};

use ep_core::Options;
use ep_metal::show::Show;
use ep_metal::surface::Surface;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_foundation::{
    NSDefaultRunLoopMode, NSDistributedNotificationCenter, NSNotification, NSRect, NSRunLoop,
    ns_string,
};
use objc2_quartz_core::CADisplayLink;
use objc2_screen_saver::ScreenSaverView;

struct SaverState {
    show: Show,
    surface: Surface,
    /// Drives `step:` while the host has the saver animating.
    link: Option<Retained<CADisplayLink>>,
}

/// Gives each instance its own seed, like separate ep processes started at
/// slightly different times.
static INSTANCES: AtomicI64 = AtomicI64::new(0);

define_class!(
    #[unsafe(super(ScreenSaverView, NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "EPSaverView"]
    #[ivars = RefCell<Option<SaverState>>]
    struct SaverView;

    impl SaverView {
        #[unsafe(method_id(initWithFrame:isPreview:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect, is_preview: Bool) -> Option<Retained<Self>> {
            let this = this.set_ivars(RefCell::new(None));
            // SAFETY: ScreenSaverView's designated initialiser, once, on a fresh
            // allocation.
            let view: Option<Retained<Self>> =
                unsafe { msg_send![super(this), initWithFrame: frame, isPreview: is_preview] };
            if let Some(view) = &view {
                view.setup();
            }
            view
        }

        /// Frames come from the display link instead (`step:`), at the
        /// display's own refresh rate.
        #[unsafe(method(animateOneFrame))]
        fn animate_one_frame(&self) {}

        #[unsafe(method(step:))]
        fn step(&self, link: &CADisplayLink) {
            self.advance(link.targetTimestamp());
        }

        #[unsafe(method(startAnimation))]
        fn start_animation(&self) {
            // SAFETY: ScreenSaverView's own `startAnimation`, which overrides
            // must call.
            let _: () = unsafe { msg_send![super(self), startAnimation] };
            if let Some(st) = self.ivars().borrow_mut().as_mut()
                && st.link.is_none()
            {
                st.link = Some(self.display_link());
            }
        }

        /// The host stops animating when a preview is hidden or the saver
        /// ends. Invalidating the display link stops the frames and lets go
        /// of this view, which the link retains.
        #[unsafe(method(stopAnimation))]
        fn stop_animation(&self) {
            // SAFETY: ScreenSaverView's own `stopAnimation`, which overrides
            // must call.
            let _: () = unsafe { msg_send![super(self), stopAnimation] };
            let link = self.ivars().borrow_mut().as_mut().and_then(|st| st.link.take());
            if let Some(link) = link {
                link.invalidate();
            }
        }

        /// The legacyScreenSaver host on recent macOS may keep instances
        /// alive after the saver is dismissed; quitting on its "will stop"
        /// notification is the established workaround.
        #[unsafe(method(screensaverWillStop:))]
        fn screensaver_will_stop(&self, _note: &NSNotification) {
            // SAFETY: a property read on this (ScreenSaverView) instance.
            if !unsafe { self.isPreview() } {
                std::process::exit(0);
            }
        }
    }
);

impl SaverView {
    fn setup(&self) {
        match Surface::new() {
            Ok(surface) => self.start(surface),
            // The view stays black: a panic here would take down the host
            // (System Settings or the screen saver process).
            Err(e) => eprintln!("electropaint: {e}"),
        }

        // SAFETY: `screensaverWillStop:` is defined on this class and takes the
        // notification.
        unsafe {
            NSDistributedNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self as &AnyObject,
                sel!(screensaverWillStop:),
                Some(ns_string!("com.apple.screensaver.willstop")),
                None,
            );
        }
    }

    /// Adopts `surface`'s layer and runs a show on it from the display link.
    fn start(&self, surface: Surface) {
        self.setLayer(Some(surface.layer()));
        self.setWantsLayer(true);

        let seed = Options::clock_seed() + INSTANCES.fetch_add(1, Ordering::Relaxed);
        let link = self.display_link();
        let show = Show::new(Options::new(seed).saver());
        *self.ivars().borrow_mut() = Some(SaverState {
            show,
            surface,
            link: Some(link),
        });
    }

    /// A display link calling `step:` at this view's display's refresh rate.
    fn display_link(&self) -> Retained<CADisplayLink> {
        // SAFETY: `step:` is defined on this class and takes the `CADisplayLink`.
        let link = unsafe { self.displayLinkWithTarget_selector(self as &AnyObject, sel!(step:)) };
        // SAFETY: the main run loop, from the main thread (`MainThreadOnly`).
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSDefaultRunLoopMode) };
        link
    }

    fn advance(&self, time: f64) {
        let mut state = self.ivars().borrow_mut();
        let Some(st) = state.as_mut() else { return };
        let scale = self.window().map_or(1.0, |w| w.backingScaleFactor());
        st.surface.fit(self.bounds().size, scale);
        st.surface.render(&mut st.show, time);
    }
}

/// Registers `EPSaverView` with the Objective-C runtime when the bundle is
/// loaded, before the host asks for its principal class by name.
extern "C" fn register_class() {
    let _ = SaverView::class();
}

#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static REGISTER: extern "C" fn() = register_class;
