//! electropaint as a macOS screen saver.
//!
//! The bundle's principal class, `EPSaverView`, is a `ScreenSaverView` that
//! hosts a `CAMetalLayer` and runs one show per instance (one per display,
//! plus the small preview in System Settings).

use std::cell::RefCell;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ep_core::Options;
use ep_metal::Renderer;
use ep_metal::show::Show;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_metal::MTLCreateSystemDefaultDevice;
use objc2_foundation::{
    NSDefaultRunLoopMode, NSDistributedNotificationCenter, NSNotification, NSRect, NSRunLoop, NSSize,
    ns_string,
};
use objc2_quartz_core::{CADisplayLink, CAMetalLayer};
use objc2_screen_saver::ScreenSaverView;

struct SaverState {
    show: Show,
    renderer: Renderer,
    layer: Retained<CAMetalLayer>,
    _link: Retained<CADisplayLink>,
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

        /// The legacyScreenSaver host on recent macOS may keep instances
        /// alive after the saver is dismissed; quitting on its "will stop"
        /// notification is the established workaround.
        #[unsafe(method(screensaverWillStop:))]
        fn screensaver_will_stop(&self, _note: &NSNotification) {
            if !unsafe { self.isPreview() } {
                std::process::exit(0);
            }
        }
    }
);

impl SaverView {
    fn setup(&self) {
        let device = MTLCreateSystemDefaultDevice().expect("no Metal device");
        let renderer = Renderer::new(device);
        let layer = CAMetalLayer::new();
        renderer.configure_layer(&layer);
        self.setLayer(Some(&layer));
        self.setWantsLayer(true);

        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let seed = now + INSTANCES.fetch_add(1, Ordering::Relaxed);
        let link = unsafe { self.displayLinkWithTarget_selector(self as &AnyObject, sel!(step:)) };
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSDefaultRunLoopMode) };
        let show = Show::new(Options::new(seed).saver());
        *self.ivars().borrow_mut() = Some(SaverState { show, renderer, layer, _link: link });

        unsafe {
            NSDistributedNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self as &AnyObject,
                sel!(screensaverWillStop:),
                Some(ns_string!("com.apple.screensaver.willstop")),
                None,
            );
        }
    }

    fn advance(&self, time: f64) {
        let mut state = self.ivars().borrow_mut();
        let Some(st) = state.as_mut() else { return };
        let scale = self.window().map_or(1.0, |w| w.backingScaleFactor());
        let bounds = self.bounds().size;
        let size = NSSize::new(bounds.width * scale, bounds.height * scale);
        if st.layer.drawableSize() != size {
            st.layer.setContentsScale(scale);
            st.layer.setDrawableSize(size);
        }
        st.show.render(&mut st.renderer, &st.layer, time);
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
