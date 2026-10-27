//! One running electropaint show: the simulation paced as the original was,
//! at 60 Hz, with in-between pictures for faster displays.

use ep_core::frame::Frame;
use ep_core::{Options, Sim};

use crate::Renderer;

/// ep's only pacing was `swapbuffers`, which waits for vertical retrace on a
/// 60 Hz SGI display: one swap per retrace.
pub const TICK_HZ: f64 = 60.0;

/// If a show falls this many retraces behind (e.g. after sleep), it skips
/// ahead instead of fast-forwarding.
const MAX_CATCH_UP: f64 = 4.0;

/// In-betweens closer than this to a real step aren't worth drawing.
const MIN_ALPHA: f64 = 0.02;

pub struct Show {
    sim: Sim,
    /// The next tick, computed ahead and waiting for its time, with the
    /// number of retraces it takes (one per swap; smooth smear swaps twice).
    pending: Option<(Frame, u32)>,
    /// Retrace time at which the last shown tick finished.
    shown: f64,
    start: Option<f64>,
}

/// What a display refresh should show.
pub enum Picture {
    /// Real ticks that are due, to be executed in order.
    Ticks(Vec<Frame>),
    /// A picture between the last real tick and the next.
    Between(Frame),
    /// Nothing new.
    Same,
}

impl Show {
    pub fn new(opts: Options) -> Self {
        Show {
            sim: Sim::new(opts),
            pending: None,
            shown: 0.0,
            start: None,
        }
    }

    /// What to show on a refresh at `time` seconds (any monotonic clock,
    /// ideally when the refresh will reach the screen).
    pub fn picture(&mut self, time: f64) -> Picture {
        let start = *self.start.get_or_insert(time);
        let now = (time - start) * TICK_HZ;
        if now - self.shown > MAX_CATCH_UP {
            self.shown = now - MAX_CATCH_UP;
        }
        let mut due = Vec::new();
        loop {
            let (frame, retraces) = match self.pending.take() {
                Some(pending) => pending,
                None if self.sim.exited() => break,
                None => {
                    let frame = self.sim.tick();
                    // A tick with no swap (rare) still takes a retrace, so time moves.
                    let retraces = frame.swap_count().max(1) as u32;
                    (frame, retraces)
                }
            };
            if self.shown + f64::from(retraces) > now + 1e-6 {
                self.pending = Some((frame, retraces));
                break;
            }
            due.push(frame);
            self.shown += f64::from(retraces);
        }
        if !due.is_empty() {
            return Picture::Ticks(due);
        }
        // The simulation is one tick ahead of the screen: blend into it.
        let Some((_, retraces)) = &self.pending else {
            return Picture::Same;
        };
        let alpha = (now - self.shown) / f64::from(*retraces);
        if alpha < MIN_ALPHA {
            return Picture::Same;
        }
        match self.sim.frame_between(alpha.min(1.0) as f32) {
            Some(frame) => Picture::Between(frame),
            None => Picture::Same,
        }
    }

    /// Advances to `time` and renders onto `layer`.
    pub(crate) fn render(
        &mut self,
        renderer: &mut Renderer,
        layer: &objc2_quartz_core::CAMetalLayer,
        time: f64,
    ) {
        match self.picture(time) {
            Picture::Ticks(frames) => renderer.render(layer, &frames),
            Picture::Between(frame) => renderer.render_between(layer, &frame),
            Picture::Same => {}
        }
    }

    pub fn sim(&self) -> &Sim {
        &self.sim
    }

    /// For input. Inputs take effect at the tick after the one already
    /// computed ahead, i.e. within two ticks.
    pub fn sim_mut(&mut self) -> &mut Sim {
        &mut self.sim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(show: &mut Show, times: impl Iterator<Item = f64>) -> String {
        times
            .map(|t| match show.picture(t) {
                Picture::Ticks(f) => format!("T{}", f.len()),
                Picture::Between(_) => "b".into(),
                Picture::Same => ".".into(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn sixty_hz_shows_every_tick_once() {
        let mut show = Show::new(Options::new(1));
        // Warm up past the startup swaps.
        kinds(&mut show, (0..20).map(|i| i as f64 / 60.0));
        let k = kinds(&mut show, (20..30).map(|i| i as f64 / 60.0));
        assert_eq!(k, "T1 T1 T1 T1 T1 T1 T1 T1 T1 T1");
    }

    #[test]
    fn one_twenty_hz_alternates_ticks_and_in_betweens() {
        let mut show = Show::new(Options::new(1));
        kinds(&mut show, (0..40).map(|i| i as f64 / 120.0));
        let k = kinds(&mut show, (40..48).map(|i| i as f64 / 120.0));
        assert_eq!(k, "T1 b T1 b T1 b T1 b");
    }
}
