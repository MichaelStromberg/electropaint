//! electropaint as a macOS screen saver.
//!
//! The bundle's principal class is a `ScreenSaverView` that hosts a
//! `CAMetalLayer` and runs a show per instance (one per display, plus the
//! small preview in System Settings). Built with the `remastered` feature
//! it is a second saver, `EPRemasteredSaverView`, which draws in the
//! remastered style with as many strands as fit each display. The class
//! names differ because System Settings can load both savers at once.

use std::cell::RefCell;
use std::sync::atomic::{AtomicI64, Ordering};

use ep_core::Options;
use ep_metal::show::Show;
use ep_metal::surface::{Style, Surface, auto_strands};
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

#[cfg(not(feature = "remastered"))]
const STYLE: Style = Style::Classic;
#[cfg(feature = "remastered")]
const STYLE: Style = Style::Remastered;

struct SaverState {
    /// One per strand, left to right.
    shows: Vec<Show>,
    surface: Surface,
    /// Drives `step:` while the host has the saver animating.
    link: Option<Retained<CADisplayLink>>,
}

/// Named so rust-analyzer can read it inside `define_class!`, which it
/// cannot do for a generic type written in place.
type SaverIvars = RefCell<Option<SaverState>>;

/// Gives each instance its own seed, like separate ep processes started at
/// slightly different times.
static INSTANCES: AtomicI64 = AtomicI64::new(0);

/// Defines the view class under `$name`, which differs between the two
/// savers.
macro_rules! saver_view {
    ($name:literal) => {
        define_class!(
            #[unsafe(super(ScreenSaverView, NSView))]
            #[thread_kind = MainThreadOnly]
            #[name = $name]
            #[ivars = SaverIvars]
            struct SaverView;

            impl SaverView {
                #[unsafe(method_id(initWithFrame:isPreview:))]
                fn init_with_frame(this: Allocated<Self>, frame: NSRect, is_preview: Bool) -> Option<Retained<Self>> {
                    let this = this.set_ivars(RefCell::new(None));
                    let view: Option<Retained<Self>> =
                        // SAFETY: ScreenSaverView's designated initialiser, once, on a
                        // fresh allocation.
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
    };
}

#[cfg(not(feature = "remastered"))]
saver_view!("EPSaverView");
#[cfg(feature = "remastered")]
saver_view!("EPRemasteredSaverView");

impl SaverView {
    fn setup(&self) {
        match Surface::new(STYLE) {
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

        let size = self.bounds().size;
        let strands = match STYLE {
            Style::Classic => 1,
            Style::Remastered => auto_strands(size.width, size.height),
        };
        let first = INSTANCES.fetch_add(strands as i64, Ordering::Relaxed);
        let shows = (0..strands)
            .map(|i| {
                let mut opts = Options::new(Options::clock_seed() + first + i as i64).saver();
                opts.scene = STYLE == Style::Remastered;
                Show::new(opts)
            })
            .collect();
        let link = self.display_link();
        *self.ivars().borrow_mut() = Some(SaverState {
            shows,
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
        let window = self.window();
        let scale = window.as_ref().map_or(1.0, |w| w.backingScaleFactor());
        st.surface.fit(self.bounds().size, scale);
        let headroom = window
            .and_then(|w| w.screen())
            .map_or(1.0, |s| s.maximumExtendedDynamicRangeColorComponentValue());
        st.surface.set_headroom(headroom);
        st.surface.render(&mut st.shows, time);
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
