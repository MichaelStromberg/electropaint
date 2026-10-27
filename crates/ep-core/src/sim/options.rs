//! How a run is set up, and why it swaps.

use crate::gl::RgbColorIndex;
use crate::script::Script;

/// How a run is set up (the original's command-line flags and hardware).
#[derive(Clone, Debug)]
pub struct Options {
    /// `srand48` seed; the original uses `time(NULL)`.
    pub seed: i64,
    /// `-q`: four mirrored strands.
    pub quad: bool,
    /// `-S`: screensaver mode (no window changes, no colour-map ramp).
    pub saver: bool,
    /// The script to run: the built-in show (default, `-b`), one read
    /// with `-a`, or `None` for `-c` (no script, controls visible).
    pub script: Option<Script>,
    /// `-o` teach mode: releasing a control with "auto" on records its
    /// limits into the random walk.
    pub record: bool,
    /// Screen size as `getgdesc(GD_XPMAX/GD_YPMAX)` reports it; the
    /// full-screen aspect ratio is its ratio.
    pub screen: (i32, i32),
    /// Deliver the window system's REDRAW on the first tick, as on the
    /// traced machine (non-saver mode). Turn off to inject REDRAWs with
    /// [`Sim::redraw`](crate::Sim::redraw) instead.
    pub startup_redraw: bool,
    /// Record an [`IrisCall`](crate::IrisCall) log.
    pub call_log: bool,
    /// What `color()` does in RGB mode (only the fade uses it).
    pub rgb_color_index: RgbColorIndex,
}

impl Options {
    /// The default show (`ep` with no flags) with the given seed.
    pub fn new(seed: i64) -> Options {
        Options {
            seed,
            quad: false,
            saver: false,
            script: Some(Script::builtin()),
            record: false,
            screen: (1280, 1024),
            startup_redraw: true,
            call_log: false,
            rgb_color_index: RgbColorIndex::PackedLow16,
        }
    }

    /// `-S`: screensaver mode (no startup REDRAW either, as traced).
    pub fn saver(mut self) -> Options {
        self.saver = true;
        self.startup_redraw = false;
        self
    }

    /// The seed the original uses, `time(NULL)`: seconds since the epoch.
    pub fn clock_seed() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    }
}

impl Default for Options {
    /// The default show seeded from the clock, as the original does.
    fn default() -> Options {
        Options::new(Options::clock_seed())
    }
}

/// Why a `swapbuffers` happened. Ticks normally present once, but the
/// original also swaps at startup, twice when changing between window and
/// full screen, twice per tick in smooth mode, and while stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapCause {
    /// The pre-loop clear (the original's trace frame 0).
    Startup,
    /// `fullscreen()`/`regularscreen()` clearing both buffers.
    ScreenChange,
    /// Smear+smooth: the first of the tick's two draws.
    SmoothFirstPass,
    /// The stop button: a redraw, or an idle swap while stopped.
    Stopped,
    /// The tick's frame.
    Frame,
}
