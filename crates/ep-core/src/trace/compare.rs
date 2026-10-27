//! Running the simulation alongside a decoded trace and comparing calls.

use std::collections::VecDeque;
use std::fmt::Write as _;

use super::{TraceFrame, is_draw_call};
use crate::gl::{Arg, IrisCall};
use crate::sim::{Options, Sim, SwapCause};

/// Comparison tolerance for floats. Integers (including every `rotate`
/// angle) must match exactly.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    /// Relative tolerance.
    pub rel: f64,
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance { rel: 1e-5 }
    }
}

fn close(a: f64, b: f64, tol: Tolerance) -> bool {
    a == b || (a - b).abs() <= tol.rel * a.abs().max(b.abs()).max(1e-6)
}

/// Statistics for the float arguments compared.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FloatStats {
    pub compared: u64,
    /// Floats whose bits differ but are within tolerance (−0 vs +0 counts).
    pub inexact: u64,
    pub max_rel: f64,
}

/// Compare a trace call with a simulated one; `Err` describes the first
/// difference. A NaN on the simulated side matches anything (the original
/// read uninitialised memory there).
pub(crate) fn compare_call(
    trace: &IrisCall,
    sim: &IrisCall,
    tol: Tolerance,
    st: &mut FloatStats,
) -> Result<(), String> {
    if trace.name != sim.name {
        return Err(format!("call {} vs {}", trace.name, sim.name));
    }
    if trace.args.len() != sim.args.len() {
        return Err(format!(
            "{}: {} vs {} args",
            trace.name,
            trace.args.len(),
            sim.args.len()
        ));
    }
    let mut float =
        |i: usize, j: Option<usize>, a: f64, b: f64, same_bits: bool| -> Result<(), String> {
            if b.is_nan() {
                return Ok(());
            }
            st.compared += 1;
            if !same_bits {
                st.inexact += 1;
                if a != b {
                    st.max_rel = st
                        .max_rel
                        .max((a - b).abs() / a.abs().max(b.abs()).max(1e-30));
                }
            }
            if close(a, b, tol) {
                Ok(())
            } else {
                let at = j.map_or_else(|| format!("arg {i}"), |j| format!("arg {i}[{j}]"));
                Err(format!("{}: {at}: {a:e} vs {b:e}", trace.name))
            }
        };
    for (i, (a, b)) in trace.args.iter().zip(&sim.args).enumerate() {
        match (a, b) {
            (_, Arg::Ptr) | (Arg::Ptr, _) => {}
            (Arg::Int(x), Arg::Int(y)) => {
                if x != y {
                    return Err(format!("{}: arg {i}: {x} vs {y}", trace.name));
                }
            }
            (Arg::Float(x), Arg::Float(y)) => float(
                i,
                None,
                f64::from(*x),
                f64::from(*y),
                x.to_bits() == y.to_bits(),
            )?,
            (Arg::Double(x), Arg::Double(y)) => float(i, None, *x, *y, x.to_bits() == y.to_bits())?,
            (Arg::Floats(x), Arg::Floats(y)) => {
                if x.len() != y.len() {
                    return Err(format!(
                        "{}: arg {i}: {} vs {} floats",
                        trace.name,
                        x.len(),
                        y.len()
                    ));
                }
                for (j, (p, q)) in x.iter().zip(y).enumerate() {
                    float(
                        i,
                        Some(j),
                        f64::from(*p),
                        f64::from(*q),
                        p.to_bits() == q.to_bits(),
                    )?;
                }
            }
            _ => return Err(format!("{}: arg {i}: type {a:?} vs {b:?}", trace.name)),
        }
    }
    Ok(())
}

/// Where the simulation first left the trace.
#[derive(Clone, Debug)]
pub struct Divergence {
    pub frame: usize,
    /// Index of the differing call within the frame.
    pub call: usize,
    pub message: String,
    /// The frame's calls on both sides, for context.
    pub trace: Vec<IrisCall>,
    pub sim: Vec<IrisCall>,
}

impl Divergence {
    /// A readable excerpt: `context` calls either side of the difference.
    pub fn render(&self, context: usize) -> String {
        let mut s = format!(
            "frame {}, call {}: {}\n",
            self.frame, self.call, self.message
        );
        let lo = self.call.saturating_sub(context);
        for (label, calls) in [("trace", &self.trace), ("sim", &self.sim)] {
            let _ = writeln!(s, "  {label}:");
            for (i, c) in calls.iter().enumerate().skip(lo).take(2 * context + 1) {
                let _ = writeln!(
                    s,
                    "  {} {i:5} {c}",
                    if i == self.call { ">>" } else { "  " }
                );
            }
        }
        s
    }
}

/// The result of comparing a run with a trace.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Frames that matched (a trailing incomplete frame counts once its
    /// recorded prefix matched).
    pub frames: usize,
    /// Trace frames available for comparison.
    pub available: usize,
    pub calls: u64,
    pub floats: FloatStats,
    pub divergence: Option<Divergence>,
    /// A trailing incomplete frame was compared as a prefix.
    pub partial_last: bool,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.divergence.is_none() && self.frames == self.available
    }
}

/// Run the simulation alongside a decoded trace and compare frame by frame,
/// for at most `max_frames` trace frames. REDRAW events are replayed where
/// the trace has them. Comparison stops before the first frame with mouse
/// input.
pub fn compare(
    trace: &[TraceFrame],
    mut opts: Options,
    max_frames: usize,
    tol: Tolerance,
) -> Report {
    opts.call_log = true;
    opts.startup_redraw = false;
    let mut sim = Sim::new(opts);
    let limit = trace
        .iter()
        .position(|f| f.mouse)
        .unwrap_or(trace.len())
        .min(max_frames);
    let mut rep = Report {
        available: limit,
        ..Report::default()
    };
    let mut pending: VecDeque<Vec<IrisCall>> = VecDeque::new();
    let mut cur: Vec<IrisCall> = Vec::new();
    // Frames produced by the simulation so far.
    let mut produced = 0;
    let mut absorb =
        |sim: &mut Sim, pending: &mut VecDeque<Vec<IrisCall>>, produced: &mut usize| {
            for c in sim.take_calls() {
                if !is_draw_call(&c) {
                    continue;
                }
                let swap = c.name == "swapbuffers";
                cur.push(c);
                if swap {
                    pending.push_back(std::mem::take(&mut cur));
                    *produced += 1;
                }
            }
        };
    absorb(&mut sim, &mut pending, &mut produced);
    for f in 0..limit {
        while pending.is_empty() && !sim.exited() {
            if redraw_due(&sim, &trace[..limit], produced) {
                sim.redraw();
            }
            sim.tick();
            absorb(&mut sim, &mut pending, &mut produced);
        }
        let t = &trace[f];
        let s = pending.pop_front().unwrap_or_default();
        let n = if t.complete {
            t.calls.len().max(s.len())
        } else {
            t.calls.len()
        };
        for i in 0..n {
            let res = match (t.calls.get(i), s.get(i)) {
                (Some(a), Some(b)) => compare_call(a, b, tol, &mut rep.floats),
                (Some(a), None) => Err(format!("trace has {a}, sim has nothing")),
                (None, Some(b)) => Err(format!("sim has {b}, trace has nothing")),
                (None, None) => Ok(()),
            };
            if let Err(message) = res {
                rep.divergence = Some(Divergence {
                    frame: f,
                    call: i,
                    message,
                    trace: t.calls.clone(),
                    sim: s,
                });
                return rep;
            }
            rep.calls += 1;
        }
        rep.partial_last |= !t.complete;
        rep.frames += 1;
    }
    rep
}

/// Whether the simulation's next tick should see a REDRAW: the trace reads
/// one in a frame that tick will produce. `produced` frames are already
/// accounted for.
fn redraw_due(sim: &Sim, trace: &[TraceFrame], produced: usize) -> bool {
    let limit = trace.len();
    // A REDRAW belongs to the tick whose frames contain it; a tick
    // produces at most a handful of frames.
    let window = &trace[produced.min(limit)..limit.min(produced + 6)];
    if !window.iter().any(|t| t.redraw) {
        return false;
    }
    let mut probe = sim.clone();
    probe.tick();
    // The startup swap was logged when the simulation was created.
    let n = probe
        .swap_causes()
        .iter()
        .filter(|c| **c != SwapCause::Startup)
        .count();
    trace[produced.min(limit)..limit.min(produced + n)]
        .iter()
        .any(|t| t.redraw)
}
