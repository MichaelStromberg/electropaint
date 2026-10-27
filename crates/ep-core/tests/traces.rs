//! The simulation's IrisGL call stream against traces of the real program
//! (IRIX 5.3 `ep` on an emulated Indy, `time()` = 12345).
//!
//! The traces live in the git-ignored `re/` directory; each test passes
//! trivially (with a note on stderr) when its files are absent, unless
//! `EP_REQUIRE_TRACES` is set.

use std::path::{Path, PathBuf};

use ep_core::trace::{self, Tolerance};
use ep_core::{Options, Script};

fn re() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re")
}

/// A test's files are absent: skip it, or fail if `EP_REQUIRE_TRACES` is set.
fn missing(what: &str) {
    assert!(
        std::env::var_os("EP_REQUIRE_TRACES").is_none(),
        "{what}, and EP_REQUIRE_TRACES is set"
    );
    eprintln!("skipping {what}");
}

/// One recording and how it was made.
struct Case {
    /// `re/tracer/<trace>.trace`.
    trace: &'static str,
    /// `re/scripts/<script>`, fed to `ep -a`; `None` for the built-in show.
    script: Option<&'static str>,
    quad: bool,
    saver: bool,
    /// How many frames must match (the whole trace, unless it is cut
    /// short by a capture problem).
    frames: usize,
}

impl Case {
    fn new(trace: &'static str, frames: usize) -> Case {
        Case {
            trace,
            script: None,
            quad: false,
            saver: false,
            frames,
        }
    }

    fn script(mut self, s: &'static str) -> Case {
        self.script = Some(s);
        self
    }

    fn quad(mut self) -> Case {
        self.quad = true;
        self
    }

    fn saver(mut self) -> Case {
        self.saver = true;
        self
    }

    fn check(self) {
        let dir = re().join("tracer");
        let ops = dir.join("eptrace_ops.json");
        let path = dir.join(format!("{}.trace", self.trace));
        if !ops.exists() || !path.exists() {
            missing(&format!("{}: trace not present", self.trace));
            return;
        }
        let mut opts = Options::new(12345);
        opts.quad = self.quad;
        if self.saver {
            opts = opts.saver();
        }
        if let Some(s) = self.script {
            let p = re().join("scripts").join(s);
            let Ok(text) = std::fs::read_to_string(&p) else {
                missing(&format!("{}: {} not present", self.trace, p.display()));
                return;
            };
            opts.script = Some(Script::parse(&text));
        }
        let data = trace::load(&ops, &path).unwrap();
        let rep = trace::compare(&data, opts, self.frames, Tolerance::default());
        if let Some(d) = &rep.divergence {
            panic!("{}: {}", self.trace, d.render(6));
        }
        assert_eq!(rep.frames, self.frames, "{}: frames compared", self.trace);
        // Not only within tolerance: every float is bit-identical.
        assert_eq!(
            rep.floats.inexact, 0,
            "{}: floats not bit-identical",
            self.trace
        );
    }
}

#[test]
fn default_200() {
    Case::new("ep200", 200).check();
    Case::new("ep200b", 200).check();
    Case::new("ep5", 5).check();
}

#[test]
fn default_3600() {
    Case::new("ep_long", 3600).check();
}

/// The built-in script fed back as text with `-a`.
#[test]
fn builtin_as_text() {
    Case::new("ep_a", 200).script("mello.ep").check();
}

#[test]
fn quad() {
    Case::new("ep_q", 300).quad().check();
}

#[test]
fn screensaver() {
    Case::new("ep_S", 300).saver().check();
}

#[test]
fn ribbons_zbuffer_light() {
    Case::new("ep_rzl", 300).script("mello-rzl.ep").check();
}

#[test]
fn variants() {
    for (trace, script) in [
        ("ep_v_light", "mello-light.ep"),
        ("ep_v_zl", "mello-zl.ep"),
        ("ep_v_smear", "mello-smear.ep"),
        ("ep_v_smoothsmear", "mello-smoothsmear.ep"),
        ("ep_v_smearfade", "mello-smearfade.ep"),
        ("ep_v_back", "mello-back.ep"),
        ("ep_v_tri", "mello-tri.ep"),
        ("ep_v_fat", "mello-fat.ep"),
    ] {
        Case::new(trace, 300).script(script).check();
    }
}

/// A mouse click left full screen at frame 261 during the capture; the
/// comparison stops there.
#[test]
fn ribbons_triangles() {
    Case::new("ep_v_rzltri", 261)
        .script("mello-rzltri.ep")
        .check();
}

/// Indy libgl crashed in frame 188, so that frame is partial.
#[test]
fn quad_ribbons() {
    Case::new("ep_v_rzl-q", 189)
        .script("mello-rzl.ep")
        .quad()
        .check();
}
