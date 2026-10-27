//! Reading IRIX `eptrace` recordings (EPT1) and comparing them with the
//! simulation's call log.
//!
//! An EPT1 file is the magic `EPT1` followed by records: a big-endian u16
//! opcode, a u16 payload length and the payload, which holds the call's
//! arguments in MIPS (big-endian) order. The opcode table
//! (`eptrace_ops.json`, written by `tools/irix/gen_tracer.py`) gives each
//! opcode's name, parameter types, which pointer parameters were logged by
//! value (and how many floats), output parameters and return type. Frames
//! end at `swapbuffers`.
//!
//! Only the drawing stream is compared (`is_draw_call`): window, event
//! and panel calls are skipped, as is the panel library's own pattern.
//! Window-system REDRAW events are inputs, so the comparison replays them
//! where the trace shows the main loop reading one.

mod compare;
mod decode;
mod json;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::gl::{Arg, IrisCall};
pub use compare::{Divergence, FloatStats, Report, Tolerance, compare};
use decode::{Decoder, OpsTable};

// ---------------------------------------------------------------- filtering

/// Calls that make up the drawing stream (and the random-number stream,
/// which pins down the animation). Everything else ep or libpanel calls is
/// window management, event handling, colour-map bookkeeping or panel
/// drawing into other windows.
const DRAW_CALLS: &[&str] = &[
    "srand48",
    "drand48",
    "doublebuffer",
    "mmode",
    "lmdef",
    "lmbind",
    "RGBmode",
    "blendfunction",
    "subpixel",
    "gconfig",
    "shademodel",
    "concave",
    "glcompat",
    "perspective",
    "loadmatrix",
    "multmatrix",
    "polarview",
    "lsetdepth",
    "reshapeviewport",
    "c4f",
    "cpack",
    "color",
    "clear",
    "zclear",
    "zbuffer",
    "swapbuffers",
    "pushmatrix",
    "popmatrix",
    "rotate",
    "translate",
    "scale",
    "linewidth",
    "n3f",
    "v2f",
    "v3f",
    "t2f",
    "bgnpolygon",
    "endpolygon",
    "bgnclosedline",
    "endclosedline",
    "bgntmesh",
    "endtmesh",
    "frontbuffer",
    "backbuffer",
    "defpattern",
    "setpattern",
    "mapcolor",
];

/// Whether a call belongs to the compared drawing stream. Two names are
/// only partly compared: `mapcolor` only for the five channel swatches ep
/// sets every frame (0x300..0x304; the rest is panel colour-map
/// bookkeeping), and `defpattern` not for libpanel's own pattern 1.
fn is_draw_call(c: &IrisCall) -> bool {
    if !DRAW_CALLS.contains(&c.name) {
        return false;
    }
    match (c.name, c.args.first()) {
        ("mapcolor", Some(Arg::Int(i))) => (0x300..0x305).contains(i),
        ("defpattern", Some(Arg::Int(i))) => *i != 1,
        _ => true,
    }
}

/// One frame of a trace: its drawing calls, whether it ended with a swap,
/// and the input events the main loop saw in it.
#[derive(Clone, Debug, Default)]
pub struct TraceFrame {
    pub calls: Vec<IrisCall>,
    pub complete: bool,
    /// The main loop handled a window-system REDRAW.
    pub redraw: bool,
    /// A mouse button event was read: user input the comparison does not
    /// replay.
    pub mouse: bool,
}

/// Decode a whole trace into frames.
fn frames(ops: &OpsTable, data: &[u8]) -> Result<Vec<TraceFrame>, String> {
    let mut out = Vec::new();
    let mut cur = TraceFrame::default();
    let mut last_qread: Option<i64> = None;
    for r in Decoder::new(ops, data)? {
        let r = r?;
        if r.name == "qread" {
            let v = match r.ret {
                Some(Arg::Int(v)) => v,
                _ => -1,
            };
            if v == 103 {
                cur.mouse = true;
            }
            last_qread = Some(v);
            continue;
        }
        // The main loop's REDRAW handler calls reshapeviewport right after
        // reading the event (fullscreen() calls it after winposition).
        if r.name == "reshapeviewport" && last_qread == Some(528) {
            cur.redraw = true;
        }
        last_qread = None;
        let c = r.to_call();
        if !is_draw_call(&c) {
            continue;
        }
        let swap = c.name == "swapbuffers";
        cur.calls.push(c);
        if swap {
            cur.complete = true;
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.calls.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Why [`load`] failed.
#[derive(Debug)]
pub enum TraceError {
    /// A file could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The opcode table is malformed.
    Table(String),
    /// The trace is not a well-formed EPT1 recording.
    Trace(String),
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            TraceError::Table(msg) | TraceError::Trace(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for TraceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TraceError::Io { source, .. } => Some(source),
            TraceError::Table(_) | TraceError::Trace(_) => None,
        }
    }
}

/// Load an opcode table and a trace file.
pub fn load(
    ops_json: impl AsRef<Path>,
    trace: impl AsRef<Path>,
) -> Result<Vec<TraceFrame>, TraceError> {
    let (ops_json, trace) = (ops_json.as_ref(), trace.as_ref());
    let text = std::fs::read_to_string(ops_json).map_err(|source| TraceError::Io {
        path: ops_json.into(),
        source,
    })?;
    let ops = OpsTable::parse(&text).map_err(TraceError::Table)?;
    let data = std::fs::read(trace).map_err(|source| TraceError::Io {
        path: trace.into(),
        source,
    })?;
    frames(&ops, &data).map_err(TraceError::Trace)
}

#[cfg(test)]
mod tests {
    use super::compare::compare_call;
    use super::*;

    fn table() -> OpsTable {
        OpsTable::parse(
            r#"{"magic": "EPT1", "ops": {
              "1": {"name": "rotate", "ret": "void", "params": ["short", "char"], "extents": {}, "outs": []},
              "2": {"name": "lmdef", "ret": "void", "params": ["short", "short", "short", "float *"], "extents": {"3": "arg2"}, "outs": []},
              "3": {"name": "drand48", "ret": "double", "params": [], "extents": {}, "outs": []},
              "4": {"name": "swapbuffers", "ret": "void", "params": [], "extents": {}, "outs": []}
            }}"#,
        )
        .unwrap()
    }

    #[test]
    fn decode_records() {
        let ops = table();
        let mut d = b"EPT1".to_vec();
        d.extend([0, 1, 0, 3, 0xff, 0xf6, b'z']);
        d.extend([0, 2, 0, 14, 0, 0, 0, 1, 0, 2]);
        d.extend(1.5f32.to_be_bytes());
        d.extend(2.0f32.to_be_bytes());
        d.extend([0, 3, 0, 8]);
        d.extend(0.25f64.to_be_bytes());
        d.extend([0, 4, 0, 0]);
        let fr = frames(&ops, &d).unwrap();
        assert_eq!(fr.len(), 1);
        let c = &fr[0].calls;
        assert_eq!(
            c[0],
            IrisCall {
                name: "rotate",
                args: vec![Arg::Int(-10), Arg::Int(122)]
            }
        );
        assert_eq!(c[1].args[3], Arg::Floats(vec![1.5, 2.0]));
        assert_eq!(c[2].args, vec![Arg::Double(0.25)]);
        assert!(fr[0].complete);
    }

    #[test]
    fn nan_is_a_wildcard_and_ints_are_exact() {
        let mut st = FloatStats::default();
        let t = IrisCall {
            name: "lmdef",
            args: vec![Arg::Int(0), Arg::Floats(vec![2.0, 2.5e-29])],
        };
        let s = IrisCall {
            name: "lmdef",
            args: vec![Arg::Int(0), Arg::Floats(vec![2.0, f32::NAN])],
        };
        assert!(compare_call(&t, &s, Tolerance::default(), &mut st).is_ok());
        let s2 = IrisCall {
            name: "lmdef",
            args: vec![Arg::Int(1), Arg::Floats(vec![2.0, 0.0])],
        };
        assert!(compare_call(&t, &s2, Tolerance::default(), &mut st).is_err());
    }
}
