//! electropaint (SGI IRIX `ep`) simulation core. No graphics dependencies:
//! each tick produces a [`frame::Frame`] for a renderer to execute.
//!
//! The crate reproduces the original program's IrisGL draw stream: the
//! script sequencer and random walks (`anim`), the main loop ([`sim`]),
//! the strand drawing (`draw`) and an IrisGL emulation (`gl`) that turns
//! the calls into renderer ops and, optionally, into a call log that
//! [`trace`] compares with recordings of the real program.
//!
//! ```
//! use electropaint_core::{Options, Sim};
//! let mut sim = Sim::new(Options::new(12345));
//! let frame = sim.tick();
//! assert!(!frame.ops.is_empty());
//! ```

mod anim;
pub mod colour;
mod draw;
pub mod frame;
mod gl;
pub mod panel;
mod rand48;
pub mod scene;
pub mod script;
pub mod sim;
pub mod trace;

pub use frame::Frame;
pub use gl::{Arg, IrisCall, RgbColorIndex};
pub use panel::{Actuator, Callback, Kind, Panel, ids};
pub use script::Script;
pub use sim::{Options, Sim, SwapCause};

#[cfg(test)]
mod testing {
    /// A test's files in the git-ignored `re/` are absent: it is skipped,
    /// unless `EP_REQUIRE_TRACES` is set, which makes their absence a failure.
    pub(crate) fn re_missing(path: &str) {
        assert!(
            std::env::var_os("EP_REQUIRE_TRACES").is_none(),
            "{path} not present, and EP_REQUIRE_TRACES is set"
        );
    }
}
