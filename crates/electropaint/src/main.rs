//! electropaint for macOS: fullscreen on the main display (or every
//! display) until a key press or click, or interactive in a window or with
//! its control panel.

mod args;
mod panel;
mod snapshot;
mod view;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ep_metal::show::Show;
use ep_metal::surface::Style;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSCursor, NSMenu, NSMenuItem, NSScreen, NSWindow,
};
use objc2_foundation::NSString;

use args::{Options, parse_args};
use snapshot::{write_panel_snapshot, write_snapshot};
use view::{SHOW_LEVEL, open_window};

fn main() {
    let opts = parse_args();
    if let Some(snap) = &opts.snapshot {
        if let Err(e) = write_snapshot(snap, &opts) {
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
    app.setMainMenu(Some(&main_menu(mtm)));

    let interactive = opts.windowed || opts.controls;
    // Kept alive while the app runs, which is until it quits the process.
    let style = Rc::new(Cell::new(opts.style));
    let (_windows, panel_show) = open_show_windows(mtm, &opts, &style, interactive);
    let level = if opts.windowed { 0 } else { SHOW_LEVEL };
    // With no display there is no show, and so no panel either.
    let _panel = panel_show
        .filter(|_| interactive)
        .map(|show| panel::PanelController::new(mtm, show, style, opts.controls, level));

    #[expect(
        deprecated,
        reason = "activate() is cooperative and may not take focus from Terminal"
    )]
    app.activateIgnoringOtherApps(true);
    app.run();
}

/// The menu bar: the app menu, with Quit (⌘Q).
fn main_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let bar = NSMenu::new(mtm);
    let app_item = NSMenuItem::new(mtm);
    let app_menu = NSMenu::new(mtm);
    // SAFETY: `terminate:` is NSApplication's, which the menu item sends up
    // the responder chain to the app.
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit electropaint"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        )
    };
    app_menu.addItem(&quit);
    app_item.setSubmenu(Some(&app_menu));
    bar.addItem(&app_item);
    bar
}

/// Quits the app, which ends the process.
pub(crate) fn quit(mtm: MainThreadMarker) {
    NSApplication::sharedApplication(mtm).terminate(None);
}

/// Opens the shows: one window with `--windowed`, else a borderless
/// full-screen window on the main display (or on every display), each with
/// its strands. Also returns the show the control panel drives: the first
/// window's middle strand.
fn open_show_windows(
    mtm: MainThreadMarker,
    opts: &Options,
    style: &Rc<Cell<Style>>,
    interactive: bool,
) -> (Vec<Retained<NSWindow>>, Option<Rc<RefCell<Show>>>) {
    // Every show gets its own seed, in order across windows and strands.
    let mut seed = opts.seed_base();
    let mut new_show = || {
        let show = Show::new(opts.show_options(seed));
        seed += 1;
        Rc::new(RefCell::new(show))
    };
    let mut windows = Vec::new();
    let mut panel_show = None;
    if opts.windowed {
        let (window, main) = open_window(mtm, None, opts.strands(), &mut new_show, style, false);
        panel_show = Some(main);
        windows.push(window);
    } else {
        // The first screen is the main display (the one with the menu bar).
        let screens = NSScreen::screens(mtm);
        let count = if opts.all_displays {
            screens.len()
        } else {
            screens.len().min(1)
        };
        for screen in screens.iter().take(count) {
            let (window, main) = open_window(
                mtm,
                Some(&screen),
                opts.strands(),
                &mut new_show,
                style,
                !interactive,
            );
            panel_show.get_or_insert(main);
            windows.push(window);
        }
        if !opts.controls {
            NSCursor::hide();
        }
    }
    (windows, panel_show)
}
