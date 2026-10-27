//! In-between pictures: at the ends of a tick they reproduce the pictures the
//! simulation actually drew.

use ep_core::frame::{Frame, Op, Vertex};
use ep_core::{Options, Sim};

/// All vertices of a frame's draws, in order.
fn vertices(f: &Frame) -> Vec<Vertex> {
    f.ops
        .iter()
        .flat_map(|op| match op {
            Op::Triangles { verts, .. } | Op::Lines { verts, .. } => verts.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn max_diff(a: &[Vertex], b: &[Vertex]) -> f32 {
    assert_eq!(a.len(), b.len(), "same number of vertices");
    a.iter()
        .zip(b)
        .flat_map(|(x, y)| (0..3).map(move |i| (x.pos[i] - y.pos[i]).abs()))
        .fold(0.0, f32::max)
}

/// Runs the default show and returns the last two ticks' frames with the
/// simulation positioned after the second.
fn run(ticks: usize) -> (Sim, Frame, Frame) {
    let mut sim = Sim::new(Options::new(12345));
    let mut prev = sim.tick();
    for _ in 1..ticks - 1 {
        prev = sim.tick();
    }
    let last = sim.tick();
    (sim, prev, last)
}

#[test]
fn ends_of_a_tick_match_the_real_pictures() {
    for ticks in [50, 200, 390, 1000] {
        let (sim, prev, last) = run(ticks);
        let start = sim.frame_between(0.0).expect("default show repaints");
        let end = sim.frame_between(1.0).expect("default show repaints");
        assert!(
            max_diff(&vertices(&start), &vertices(&prev)) < 1e-4,
            "alpha 0 at tick {ticks}"
        );
        assert!(
            max_diff(&vertices(&end), &vertices(&last)) < 1e-4,
            "alpha 1 at tick {ticks}"
        );
        assert!(!start.ops.contains(&Op::Swap));
    }
}

#[test]
fn halfway_lies_between() {
    // Tick 390 is in a fast spin phase (slots move ~0.4 per tick).
    let (sim, prev, last) = run(390);
    let mid = vertices(&sim.frame_between(0.5).unwrap());
    let (a, b) = (vertices(&prev), vertices(&last));
    let span = max_diff(&a, &b);
    assert!(max_diff(&mid, &a) < span && max_diff(&mid, &b) < span);
}

#[test]
fn no_in_betweens_while_smearing() {
    let mut sim = Sim::new(Options::new(12345));
    for _ in 0..20 {
        sim.tick();
    }
    sim.key('q');
    for _ in 0..3 {
        sim.tick();
    }
    assert!(sim.frame_between(0.5).is_none());
}

#[test]
fn in_betweens_leave_the_simulation_alone() {
    let mut a = Sim::new(Options::new(7));
    let mut b = Sim::new(Options::new(7));
    for _ in 0..100 {
        let fa = a.tick();
        let fb = b.tick();
        assert_eq!(fa, fb);
        let _ = b.frame_between(0.5);
    }
}
