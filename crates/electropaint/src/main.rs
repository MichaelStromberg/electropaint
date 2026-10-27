//! electropaint for macOS: fullscreen on the main display (or every
//! display), until a key press or click.

mod panel;

use std::cell::RefCell;
use std::rc::Rc;

use ep_metal::Renderer;
use ep_metal::show::Show;
use ep_metal::surface::Surface;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSCursor, NSEvent, NSScreen,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString};
use objc2_quartz_core::CADisplayLink;

struct Options {
    windowed: bool,
    all_displays: bool,
    controls: bool,
    /// Hidden: `--panel-snapshot FILE` draws the control panel to PNGs.
    panel_snapshot: Option<String>,
    quad: bool,
    script: Option<ep_core::Script>,
    seed: Option<i64>,
    snapshot: Option<Snapshot>,
}

/// `--snapshot FILE`: render offscreen and write the picture shown after the
/// given number of 60 Hz retraces as a PNG, then exit.
struct Snapshot {
    path: String,
    frames: u64,
    width: usize,
    height: usize,
}

fn parse_args() -> Options {
    match parse(std::env::args().skip(1)) {
        Ok(Some(opts)) => opts,
        Ok(None) => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        Err(msg) => {
            eprintln!("electropaint: {msg}\n{USAGE}");
            std::process::exit(2);
        }
    }
}

/// The options `args` asks for, `None` for `--help`, or what is wrong with
/// them.
fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, String> {
    let mut opts = Options {
        windowed: false,
        all_displays: false,
        controls: false,
        panel_snapshot: None,
        quad: false,
        script: None,
        seed: None,
        snapshot: None,
    };
    let (mut frames, mut size) = (600, (1280, 1024));
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--windowed" => opts.windowed = true,
            "--all-displays" => opts.all_displays = true,
            "-c" | "--controls" => opts.controls = true,
            "--panel-snapshot" => {
                opts.panel_snapshot = Some(args.next().ok_or("--panel-snapshot needs a file")?);
            }
            "-q" | "--quad" => opts.quad = true,
            "--script" => {
                let path = args.next().ok_or("--script needs a file")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                opts.script = Some(ep_core::Script::parse(&text));
            }
            "--seed" => opts.seed = Some(number(args.next(), "--seed")?),
            "--snapshot" => {
                let path = args.next().ok_or("--snapshot needs a file")?;
                opts.snapshot = Some(Snapshot {
                    path,
                    frames: 0,
                    width: 0,
                    height: 0,
                });
            }
            "--frames" => frames = number(args.next(), "--frames")?,
            "--size" => {
                let v = args.next().unwrap_or_default();
                size = v
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .ok_or("--size takes WIDTHxHEIGHT, both above 0")?;
            }
            "-h" | "--help" => return Ok(None),
            other => return Err(format!("unknown option {other}")),
        }
    }
    if let Some(snap) = &mut opts.snapshot {
        (snap.frames, snap.width, snap.height) = (frames, size.0, size.1);
    }
    Ok(Some(opts))
}

const USAGE: &str = "usage: electropaint [--windowed | --all-displays] [--controls] [--quad] [--script FILE] [--seed N]
       electropaint --snapshot FILE.png [--quad] [--script FILE] [--seed N] [--frames N] [--size WxH]

--script runs an ep script (as `ep -a`) instead of the built-in show.
--snapshot saves the picture after --frames 60 Hz retraces (default 600)
at --size (default 1280x1024).

Fullscreen on the main display (every display with --all-displays, each
with its own show); any key or click quits.

With --windowed or --controls, the show is interactive as ep was: keys are
its shortcuts (R ribbons, Z z-buffer, L light, Q smear, ...), a click
toggles ep's full-screen state, and leaving full screen brings up the
control panel (--controls keeps it up). Esc quits.

The show steps at the original's 60 Hz; faster displays get in-between
pictures on the refreshes between steps.";

/// An option's numeric value, if it has one.
fn number<T: std::str::FromStr>(v: Option<String>, what: &str) -> Result<T, String> {
    v.and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{what} needs a number"))
}

/// Runs a show offscreen for `snap.frames` retraces and saves what is displayed.
fn write_snapshot(
    snap: &Snapshot,
    opts: ep_core::Options,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = Renderer::system_default()?;
    let mut sim = ep_core::Sim::new(opts);
    let (mut frames, mut retraces) = (Vec::new(), 0);
    while retraces < snap.frames && !sim.exited() {
        let frame = sim.tick();
        retraces += u64::from(frame.retraces());
        frames.push(frame);
    }
    let bgra = renderer.render_offscreen(snap.width, snap.height, &frames)?;
    let rgb: Vec<u8> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect();
    let file = std::io::BufWriter::new(std::fs::File::create(&snap.path)?);
    let mut encoder = png::Encoder::new(file, snap.width as u32, snap.height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

struct ViewState {
    show: Rc<RefCell<Show>>,
    surface: Surface,
    /// Screensaver-style: any key or click quits. Otherwise input goes to ep.
    quit_on_input: bool,
}

/// Named so rust-analyzer can read it inside `define_class!`, which it
/// cannot do for a generic type written in place.
type ViewIvars = RefCell<Option<ViewState>>;

define_class!(
    /// A view hosting a `CAMetalLayer` that runs one show, driven by its
    /// display's own display link (so at that display's refresh rate).
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
        show: Rc<RefCell<Show>>,
        quit_on_input: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(None));
        // SAFETY: NSView's designated initialiser, once, on a fresh allocation.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };

        let surface = Surface::new().unwrap_or_else(|e| {
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
            show,
            surface,
            quit_on_input,
        });
        view
    }

    /// Renders what the show has for a refresh at `time` seconds.
    fn advance(&self, time: f64) {
        let mut state = self.ivars().borrow_mut();
        let Some(st) = state.as_mut() else { return };

        // Keep the drawable at the view's size in device pixels.
        let scale = self.window().map_or(1.0, |w| w.backingScaleFactor());
        st.surface.fit(self.bounds().size, scale);

        let exited = {
            let mut show = st.show.borrow_mut();
            st.surface.render(&mut show, time);
            show.sim().exited()
        };
        drop(state);
        if exited {
            quit(MainThreadMarker::from(self));
        }
    }

    /// Fullscreen: any key quits. Windowed: keys are ep's own shortcuts,
    /// and Esc (or ep's exit) quits.
    fn key(&self, event: &NSEvent) {
        let chars = event
            .charactersIgnoringModifiers()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let quitting = {
            let mut state = self.ivars().borrow_mut();
            let Some(st) = state.as_mut() else { return };
            if st.quit_on_input || chars == "\u{1b}" {
                true
            } else {
                let mut show = st.show.borrow_mut();
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
                st.show.borrow_mut().sim_mut().press(ep_core::ids::MOUSE);
            }
            st.quit_on_input
        };
        if quitting {
            quit(MainThreadMarker::from(self));
        }
    }
}

/// Quits the app, which ends the process.
fn quit(mtm: MainThreadMarker) {
    NSApplication::sharedApplication(mtm).terminate(None);
}

fn show_options(opts: &Options, seed: i64) -> ep_core::Options {
    let mut o = ep_core::Options::new(seed);
    o.quad = opts.quad;
    if let Some(script) = &opts.script {
        o.script = Some(script.clone());
    }
    o
}

fn seed_base(opts: &Options) -> i64 {
    opts.seed.unwrap_or_else(ep_core::Options::clock_seed)
}

/// Opens a window running `show`: borderless and covering `screen`, above
/// the menu bar and Dock like a screen saver, or with no screen the
/// `--windowed` window. `quit_on_input` makes any key or click quit.
fn open_window(
    mtm: MainThreadMarker,
    screen: Option<&NSScreen>,
    show: Rc<RefCell<Show>>,
    quit_on_input: bool,
) -> Retained<NSWindow> {
    let (rect, style) = match screen {
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
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false,
            screen: screen,
        ]
    };
    let window: Retained<NSWindow> = Retained::into_super(window);
    // SAFETY: `Retained` owns the window, so AppKit must not release it too.
    unsafe { window.setReleasedWhenClosed(false) };
    let content = NSRect::new(NSPoint::new(0.0, 0.0), rect.size);
    let view = ShowView::new(mtm, content, show, quit_on_input);
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
    window
}

/// Window level of fullscreen shows: above the menu bar and Dock.
const SHOW_LEVEL: isize = 1000;

fn main() {
    let opts = parse_args();
    if let Some(snap) = &opts.snapshot {
        if let Err(e) = write_snapshot(snap, show_options(&opts, seed_base(&opts))) {
            eprintln!("electropaint: snapshot failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    if let Some(path) = &opts.panel_snapshot {
        write_panel_snapshot(mtm, path, &opts);
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let interactive = opts.windowed || opts.controls;
    // Kept alive while the app runs, which is until it quits the process.
    let (_windows, panel_show) = open_show_windows(mtm, &opts, interactive);
    let level = if opts.windowed { 0 } else { SHOW_LEVEL };
    // With no display there is no show, and so no panel either.
    let _panel = panel_show
        .filter(|_| interactive)
        .map(|show| panel::PanelController::new(mtm, show, opts.controls, level));

    #[expect(
        deprecated,
        reason = "activate() is cooperative and may not take focus from Terminal"
    )]
    app.activateIgnoringOtherApps(true);
    app.run();
}

/// `--panel-snapshot`: draws the control panel as it is after five seconds
/// of the show, then exits.
fn write_panel_snapshot(mtm: MainThreadMarker, path: &str, opts: &Options) {
    let show = Rc::new(RefCell::new(Show::new(show_options(opts, seed_base(opts)))));
    for i in 0..300 {
        let _ = show.borrow_mut().picture(f64::from(i) / 60.0);
    }
    let panel = panel::PanelController::new(mtm, show, false, 0);
    if let Err(e) = panel.snapshot(path) {
        eprintln!("electropaint: panel snapshot failed: {e}");
        std::process::exit(1);
    }
}

/// Opens the shows: one window with `--windowed`, else a borderless
/// full-screen window on the main display (or on every display). Also
/// returns the show the control panel drives: the (first) window's.
fn open_show_windows(
    mtm: MainThreadMarker,
    opts: &Options,
    interactive: bool,
) -> (Vec<Retained<NSWindow>>, Option<Rc<RefCell<Show>>>) {
    let base = seed_base(opts);
    let new_show = |seed| Rc::new(RefCell::new(Show::new(show_options(opts, seed))));
    let mut windows = Vec::new();
    let mut panel_show = None;
    if opts.windowed {
        let show = new_show(base);
        panel_show = Some(show.clone());
        windows.push(open_window(mtm, None, show, false));
    } else {
        // The first screen is the main display (the one with the menu bar).
        let screens = NSScreen::screens(mtm);
        let count = if opts.all_displays {
            screens.len()
        } else {
            screens.len().min(1)
        };
        for (i, screen) in screens.iter().take(count).enumerate() {
            let show = new_show(base + i as i64);
            panel_show.get_or_insert_with(|| show.clone());
            windows.push(open_window(mtm, Some(&screen), show, !interactive));
        }
        if !opts.controls {
            NSCursor::hide();
        }
    }
    (windows, panel_show)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &str) -> Result<Option<Options>, String> {
        parse(args.split_whitespace().map(String::from))
    }

    fn snapshot(args: &str) -> Option<Snapshot> {
        parsed(args).ok().flatten().and_then(|o| o.snapshot)
    }

    #[test]
    fn snapshot_takes_the_frame_and_size_options_in_any_order() {
        let s = snapshot("--snapshot a.png").expect("a snapshot");
        assert_eq!((s.frames, s.width, s.height), (600, 1280, 1024));
        let s = snapshot("--frames 5 --snapshot a.png --size 64x32").expect("a snapshot");
        assert_eq!(
            (s.path.as_str(), s.frames, s.width, s.height),
            ("a.png", 5, 64, 32)
        );
    }

    #[test]
    fn options_and_values() {
        let o = parsed("-c --quad --seed -3 --windowed").ok().flatten();
        let o = o.expect("options");
        assert!(o.controls && o.quad && o.windowed && !o.all_displays);
        assert_eq!(o.seed, Some(-3));
        assert!(o.snapshot.is_none() && o.panel_snapshot.is_none());
        assert!(matches!(parsed("--help"), Ok(None)));
    }

    #[test]
    fn malformed_options_are_errors() {
        for args in [
            "--seed",
            "--seed x",
            "--frames -1",
            "--size",
            "--size 64",
            "--size 0x5",
            "--size 5xa",
            "--snapshot",
            "--panel-snapshot",
            "--script",
            "--fast",
        ] {
            assert!(parsed(args).is_err(), "{args} should be refused");
        }
        assert_eq!(
            parsed("--fast").err().as_deref(),
            Some("unknown option --fast")
        );
    }
}
