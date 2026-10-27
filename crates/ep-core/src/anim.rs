//! The sequencer and the actuators' random walks (animation.md §2–§3):
//! `tasteQueue`, `processCommand`, `setacttargets`, `animateacts` and
//! `animatesliders`.
//!
//! Precision follows the original instruction by instruction: values the
//! original stores as `float` are `f32` here, and the double-precision
//! intermediates are `f64`. All random numbers come from `drand48`, in a
//! fixed order (§3.9), which is what makes a run reproducible.

use crate::gl::Arg;
use crate::panel::MAX_INDEX;
use crate::script::{Command, Op, Script};
use crate::sim::Sim;

/// The per-actuator random-walk record (`acttable` entry).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ActRec {
    /// Enabled for random animation (`actlim2` sets it; `actstop`,
    /// `actstopall` and turning "auto" off clear it).
    pub enabled: bool,
    /// Travelling towards `target`.
    pub moving: bool,
    pub dir: f32,
    pub target: f32,
    pub upper: f32,
    pub lower: f32,
    /// Per-frame increment.
    pub step: f32,
    pub duration: f32,
    pub variance: f32,
}

/// A sequence at run time.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SeqState {
    pub id: i32,
    /// Cleared only when the sequence is killed.
    pub active: bool,
    /// Frame origin of the command times.
    pub start: f32,
    pub looplen: f32,
    /// Copied into actuators by `actlim2`.
    pub duration: f32,
    pub variance: f32,
    pub commands: Vec<Command>,
    /// Next command to run; `None` means stopped.
    pub cur: Option<usize>,
}

impl SeqState {
    pub(crate) fn from_script(script: &Script) -> Vec<SeqState> {
        let auto = script.autostart();
        script
            .sequences
            .iter()
            .enumerate()
            .map(|(i, s)| SeqState {
                id: s.id,
                active: true,
                start: 0.0,
                looplen: s.looplen,
                duration: 300.0,
                variance: 1.0,
                commands: s.commands.clone(),
                cur: (Some(i) == auto && !s.commands.is_empty()).then_some(0),
            })
            .collect()
    }

    fn advance(&mut self) {
        self.cur = self
            .cur
            .and_then(|c| (c + 1 < self.commands.len()).then_some(c + 1));
    }

    /// The command at the cursor, if its time has come by `now`.
    fn due_command(&self, now: f64) -> Option<Command> {
        let cmd = self.commands[self.cur?];
        (f64::from(cmd.frame + self.start) < now).then_some(cmd)
    }
}

impl Sim {
    /// One `drand48()` draw (logged like the tracer logs it).
    fn drand48(&mut self) -> f64 {
        let u = self.rng.drand48();
        self.gl.log("drand48", || vec![Arg::Double(u)]);
        u
    }

    /// `exprand(1.0)`: a value in [0,1) with mean (e−2)/(e−1).
    fn exprand(&mut self) -> f32 {
        let m = 1.0f64;
        let u = self.drand48();
        ((u * m).exp_m1() / m.exp_m1()) as f32
    }

    /// `tasteQueue`: advance the sequencer clock one frame and run every
    /// due command, sequences in list order.
    pub(crate) fn taste_queue(&mut self) {
        self.current_frame += 1.0;
        let mut busy = false;
        let mut i = 0;
        while i < self.seqs.len() {
            let s = &self.seqs[i];
            if s.active && s.cur.is_some() {
                busy = true;
            }
            let now = f64::from(self.current_frame) + 0.5;
            if f64::from(s.start) < now && s.active {
                let mut due = s.due_command(now);
                while let Some(c) = due {
                    self.process_command(i, c);
                    let s = &self.seqs[i];
                    due = if s.active {
                        s.due_command(f64::from(self.current_frame) + 0.5)
                    } else {
                        None
                    };
                }
            }
            // A killed sequence is unlinked; the next one now sits at `i`.
            if self.seqs[i].active {
                i += 1;
            } else {
                self.seqs.remove(i);
            }
        }
        if !self.auto_released && !busy {
            // Release "auto" once the script is done (no callback runs).
            let auto = self.controls.act_mut(crate::panel::ids::AUTO);
            auto.val = auto.min;
            self.auto_released = true;
        }
    }

    fn find_seq(&self, id: i32) -> Option<usize> {
        self.seqs.iter().position(|s| s.id == id && s.active)
    }

    /// The actuator id of script id `id`, if `acttable` holds it (the
    /// spec's "if rec").
    fn rec_id(&self, id: i32) -> Option<u16> {
        let id = u16::try_from(id).ok()?;
        self.recs
            .get(usize::from(id))
            .and_then(|r| r.as_ref())
            .map(|_| id)
    }

    /// `processCommand` for the command of sequence `si` at its cursor.
    fn process_command(&mut self, si: usize, cmd: Command) {
        // Each arm says whether the cursor moves on: the spec's "advances
        // `s.cur`?" column.
        let advance = match cmd.op {
            Op::Frame(_) | Op::AbsFrame(_) => true,
            Op::ActLim1(id, v) => {
                if let Some(id) = self.rec_id(id) {
                    let min = self.controls.act(id).min;
                    self.rec_mut(id).lower = if v < min { min } else { v };
                }
                true
            }
            Op::ActLim2(id, v) => {
                if let Some(id) = self.rec_id(id) {
                    let (min, max) = (self.controls.act(id).min, self.controls.act(id).max);
                    let (duration, variance) = (self.seqs[si].duration, self.seqs[si].variance);
                    let r = self.rec_mut(id);
                    set_limit2(r, v, min, max);
                    r.duration = duration;
                    r.variance = variance;
                    r.enabled = true;
                }
                true
            }
            Op::ActSet(id, v) => {
                if let Some(id) = self.rec_id(id) {
                    self.setact(id, v);
                }
                true
            }
            Op::ActReset(id) => {
                if let Some(id) = self.rec_id(id) {
                    // Only `val`: for an ep slider the value bar stays put.
                    let a = self.controls.act_mut(id);
                    a.val = a.initval;
                }
                true
            }
            Op::ActResetAll => {
                self.reset();
                true
            }
            Op::ActStop(id) => {
                if let Some(id) = self.rec_id(id) {
                    self.rec_mut(id).enabled = false;
                }
                true
            }
            Op::ActStopAll => {
                self.stop_anim();
                true
            }
            Op::SeqDo(id) => {
                let cf = self.current_frame;
                let t = self.find_seq(id);
                if let Some(t) = t {
                    let ts = &mut self.seqs[t];
                    ts.cur = (!ts.commands.is_empty()).then_some(0);
                    ts.start = cf;
                }
                t != Some(si)
            }
            Op::SeqStop(id) => {
                let t = self.find_seq(id);
                if let Some(t) = t {
                    self.seqs[t].cur = None;
                }
                t != Some(si)
            }
            Op::SeqLoop => {
                let s = &mut self.seqs[si];
                s.cur = (!s.commands.is_empty()).then_some(0);
                s.start += s.looplen;
                false
            }
            // The original only kills the running sequence itself and
            // otherwise spins forever on this command; this is the intended
            // behaviour: kill the target, then go on.
            Op::SeqKill(id) => {
                if let Some(t) = self.find_seq(id) {
                    self.seqs[t].active = false;
                    self.seqs[t].cur = None;
                }
                self.seqs[si].active
            }
            // Broken in the original (kills the wrong sequence); intended:
            // kill every sequence.
            Op::SeqKillAll => {
                for s in &mut self.seqs {
                    s.active = false;
                    s.cur = None;
                }
                false
            }
            Op::Duration(v) => {
                let s = &mut self.seqs[si];
                s.duration = if f64::from(v) < 1.0 { 1.0 } else { v };
                true
            }
            Op::Variance(v) => {
                self.seqs[si].variance = v.clamp(0.0, 1.0);
                true
            }
            Op::RandDelay(d) => {
                let e = f64::from(self.exprand());
                let s = &mut self.seqs[si];
                let (var, d) = (f64::from(s.variance), f64::from(d));
                s.start = (f64::from(s.start) + (var * ((e + e) * d) + (1.0 - var) * d)) as f32;
                true
            }
        };
        if advance {
            self.seqs[si].advance();
        }
    }

    pub(crate) fn rec_mut(&mut self, id: u16) -> &mut ActRec {
        self.recs[usize::from(id)]
            .as_mut()
            .expect("actuator has an acttable record")
    }

    /// `stopAnim`: disable every random walk (`moving` is left as is).
    pub(crate) fn stop_anim(&mut self) {
        for r in self.recs.iter_mut().flatten() {
            r.enabled = false;
        }
    }

    /// The value `setacttargets`/`animateacts` treat as current: the
    /// actuator's `val` (for an ep slider, the main actuator's mirror of
    /// its value bar).
    fn act_val(&self, id: u16) -> f32 {
        let a = self.controls.act(id);
        if a.in_acttable { a.val } else { a.orphan_val }
    }

    /// `setacttargets`: every enabled idle actuator picks a uniform target
    /// in its limits and a segment length. The duration estimate uses a
    /// `MAX`-style macro that evaluates `exprand` twice, so a segment
    /// usually costs three draws, and the length used is the second
    /// `exprand`'s.
    pub(crate) fn set_act_targets(&mut self) {
        for i in 0..MAX_INDEX {
            let Some(r) = self.recs[i] else { continue };
            if !r.enabled || r.moving {
                continue;
            }
            let id = i as u16;
            let u = self.drand48() as f32;
            let target =
                (f64::from(r.upper * u) + (1.0 - f64::from(u)) * f64::from(r.lower)) as f32;
            let (dur, var) = (f64::from(r.duration), f64::from(r.variance));
            let e1 = f64::from(self.exprand());
            let mut d = (e1 + e1) * dur * var + (1.0 - var) * dur;
            if d < 1.0 {
                d = 1.0;
            } else {
                let e2 = f64::from(self.exprand());
                d = (e2 + e2) * dur * var + (1.0 - var) * dur;
            }
            let val = self.act_val(id);
            let r = self.rec_mut(id);
            r.moving = true;
            r.target = target;
            r.step = (f64::from(target - val) / d) as f32;
            r.dir = if 0.0 < f64::from(r.step) { 1.0 } else { -1.0 };
        }
    }

    /// `animateacts`: move each travelling actuator one step; on overshoot
    /// land exactly on the target and become idle.
    pub(crate) fn animate_acts(&mut self) {
        for i in 0..MAX_INDEX {
            let Some(r) = self.recs[i] else { continue };
            if !r.enabled || !r.moving {
                continue;
            }
            let id = i as u16;
            let mut v = self.act_val(id) + r.step;
            if f64::from((r.target - v) * r.dir) < 0.0 {
                self.rec_mut(id).moving = false;
                v = r.target;
            }
            self.setact(id, v);
        }
    }

    /// `animatesliders`: self-oscillation of every ep slider, in reverse
    /// creation order. The speed is read live, so sliders handled before
    /// the speed slider see its previous value.
    pub(crate) fn animate_sliders(&mut self) {
        for s in (0..self.controls.sliders.len()).rev() {
            let speed = self.controls.slider_value(crate::panel::ids::SPEED);
            self.controls.animate_slider(s, speed, self.grabbed);
        }
    }
}

/// `actlim2`'s limit logic: after an actlim1/actlim2 pair, `upper` is the
/// larger limit (at most max) and `lower` the smaller (at least min).
fn set_limit2(r: &mut ActRec, v: f32, min: f32, max: f32) {
    if r.lower < v {
        r.upper = if max < v { max } else { v };
    } else {
        r.upper = if max < r.lower { max } else { r.lower };
        r.lower = if v < min { min } else { v };
    }
}
