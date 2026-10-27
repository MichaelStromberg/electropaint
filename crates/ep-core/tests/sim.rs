//! Behaviour of the public simulation API and of the renderer frames.

use ep_core::frame::{DrawBuffer, Op};
use ep_core::{Options, Sim, SwapCause, ids};

fn swaps(ops: &[Op]) -> usize {
    ops.iter().filter(|o| **o == Op::Swap).count()
}

#[test]
fn startup_and_fullscreen_swaps() {
    let mut sim = Sim::new(Options::new(12345));
    let f = sim.tick();
    // Startup clear, the two full-screen swaps and the frame itself: trace
    // frames 0..=3.
    assert_eq!(swaps(&f.ops), 4);
    assert_eq!(
        sim.swap_causes(),
        [
            SwapCause::Startup,
            SwapCause::ScreenChange,
            SwapCause::ScreenChange,
            SwapCause::Frame
        ]
    );
    assert!(sim.fullscreen());
    assert!(!sim.panels_visible());
    let proj: Vec<_> = f
        .ops
        .iter()
        .filter_map(|o| {
            if let Op::SetProjection(p) = o {
                Some(*p)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(proj.last().unwrap().aspect, 1.25);
    let f = sim.tick();
    assert_eq!(swaps(&f.ops), 1);
    assert_eq!(sim.swap_causes(), [SwapCause::Frame]);
}

#[test]
fn saver_has_no_screen_change() {
    let mut sim = Sim::new(Options::new(12345).saver());
    sim.tick();
    assert_eq!(sim.swap_causes(), [SwapCause::Startup, SwapCause::Frame]);
    // The script still turns the full screen button on.
    assert!(sim.fullscreen());
}

#[test]
fn strand_geometry_in_eye_space() {
    let mut sim = Sim::new(Options::new(12345));
    let mut f = sim.tick();
    for _ in 0..60 {
        f = sim.tick();
    }
    assert_eq!(sim.strand().1, 40);
    let tris: usize = f
        .ops
        .iter()
        .map(|o| {
            if let Op::Triangles { verts, .. } = o {
                verts.len()
            } else {
                0
            }
        })
        .sum();
    let lines: usize = f
        .ops
        .iter()
        .map(|o| {
            if let Op::Lines { verts, .. } = o {
                verts.len()
            } else {
                0
            }
        })
        .sum();
    // 40 squares (2 triangles each); outlines, 4 segments each, for the
    // slots with the outline on.
    assert_eq!(tris, 40 * 6);
    assert!(lines > 0 && lines.is_multiple_of(8));
    for o in &f.ops {
        if let Op::Triangles { verts, state } = o {
            assert!(state.blend && !state.depth_test);
            for v in verts {
                // polarview puts the strand around z = -10.
                assert!((v.pos[2] + 10.0).abs() < 3.0, "{:?}", v.pos);
                assert!(v.color.iter().all(|c| (0.0..=1.0).contains(c)));
            }
            // Flat shading: each triangle is one colour.
            for t in verts.chunks(3) {
                assert!(t.iter().all(|v| v.color == t[2].color));
            }
        }
    }
    assert!(matches!(f.ops[0], Op::Clear(_)));
}

#[test]
fn smear_key_draws_into_both_buffers() {
    let mut sim = Sim::new(Options::new(1));
    sim.tick();
    assert!(sim.key('q'));
    let f = sim.tick();
    assert!(sim.is_on(ids::SMEAR));
    assert!(f.ops.contains(&Op::SetDrawBuffer(DrawBuffer::Both)));
    let f = sim.tick();
    assert!(
        !f.ops.iter().any(|o| matches!(o, Op::Clear(_))),
        "no clear while smearing"
    );
    sim.key('Q');
    let f = sim.tick();
    assert!(!sim.is_on(ids::SMEAR));
    assert_eq!(
        f.ops
            .iter()
            .rev()
            .find(|o| matches!(o, Op::SetDrawBuffer(_))),
        Some(&Op::SetDrawBuffer(DrawBuffer::Back))
    );
}

#[test]
fn stop_freezes_and_resumes() {
    let mut sim = Sim::new(Options::new(7));
    for _ in 0..10 {
        sim.tick();
    }
    sim.key('s');
    sim.tick();
    assert_eq!(sim.swap_causes(), [SwapCause::Stopped]);
    let n = sim.strand();
    for _ in 0..5 {
        let f = sim.tick();
        assert_eq!(f.ops, [Op::Swap]);
        assert_eq!(sim.swap_causes(), [SwapCause::Stopped]);
    }
    assert_eq!(sim.strand(), n);
    sim.key('s');
    sim.tick();
    assert_eq!(sim.swap_causes(), [SwapCause::Stopped, SwapCause::Frame]);
    assert_eq!(sim.strand().0, n.0 + 1);
}

#[test]
fn light_toggle_lights_the_squares() {
    let mut sim = Sim::new(Options::new(3));
    for _ in 0..50 {
        sim.tick();
    }
    sim.key('l');
    sim.tick();
    let f = sim.tick();
    assert!(sim.is_on(ids::LIGHT));
    let mut lit = 0;
    for o in &f.ops {
        if let Op::Triangles { verts, .. } = o {
            for v in verts {
                assert!(v.color.iter().all(|c| (0.0..=1.0).contains(c)));
                lit += 1;
            }
        }
    }
    assert_eq!(lit, 40 * 6);
}

#[test]
fn ribbons_alone_take_the_background_colour() {
    // Ribbon-only sessions never bind a lighting model; with the light on
    // there is no c4f in the loop, so the ribbon takes the background.
    let mut sim = Sim::new(Options::new(3));
    for _ in 0..50 {
        sim.tick();
    }
    sim.key('r');
    sim.tick();
    sim.key('l');
    sim.tick();
    let f = sim.tick();
    let verts: Vec<_> = f
        .ops
        .iter()
        .flat_map(|o| {
            if let Op::Triangles { verts, .. } = o {
                verts.clone()
            } else {
                vec![]
            }
        })
        .collect();
    // 40 slots, 2 vertices each: 78 triangles.
    assert_eq!(verts.len(), 78 * 3);
    assert!(verts.iter().all(|v| v.color == [0.0, 0.0, 0.0, 1.0]));
}

#[test]
fn quad_ribbons_are_four_strips() {
    let mut o = Options::new(3);
    o.quad = true;
    let mut sim = Sim::new(o);
    for _ in 0..50 {
        sim.tick();
    }
    sim.key('r');
    sim.tick();
    let f = sim.tick();
    let n: usize = f
        .ops
        .iter()
        .map(|o| {
            if let Op::Triangles { verts, .. } = o {
                verts.len()
            } else {
                0
            }
        })
        .sum();
    assert_eq!(n, 4 * 78 * 3);
}

#[test]
fn exit_key() {
    let mut sim = Sim::new(Options::new(3));
    sim.tick();
    sim.key('\x1b');
    sim.tick();
    sim.tick();
    assert!(sim.exited());
    assert!(sim.tick().ops.is_empty());
}

#[test]
fn controls_mode_runs_no_script() {
    let mut o = Options::new(3);
    o.script = None;
    let mut sim = Sim::new(o);
    for _ in 0..5 {
        sim.tick();
    }
    assert!(!sim.fullscreen());
    assert!(sim.panels_visible());
    assert_eq!(sim.strand(), (5, 32));
    // Hue channel 0 drifts by itself (rate 0.01 per frame).
    assert!((sim.value(ids::hue_slider(0)).unwrap() - 1.05).abs() < 1e-5);
    assert!(sim.take_calls().is_empty());
}

#[test]
fn set_value_moves_a_slider() {
    let mut o = Options::new(3);
    o.script = None;
    let mut sim = Sim::new(o);
    sim.tick();
    sim.set_value(ids::N, 10.7);
    sim.tick();
    assert_eq!(sim.value(ids::N), Some(10.7));
    assert_eq!(sim.strand().1, 10);
    sim.set_value(ids::N, 500.0);
    sim.tick();
    assert_eq!(sim.value(ids::N), Some(127.0));
}

#[test]
fn reset_restores_controls() {
    let mut sim = Sim::new(Options::new(3));
    sim.tick();
    sim.key('u');
    sim.tick();
    assert!(!sim.is_on(ids::FILL));
    sim.press(ids::RESET);
    sim.tick();
    sim.tick();
    assert!(sim.is_on(ids::FILL));
    // Zoom back at its initial value (the R buttons ran twice).
    assert_eq!(sim.value(ids::ZOOM), Some(0.0));
}

#[test]
fn actuator_metadata() {
    let sim = Sim::new(Options::new(3));
    let a = sim.actuator(ids::SMEAR).unwrap();
    assert_eq!(a.label, Some("smear (Q)"));
    assert_eq!(a.key, Some('Q'));
    assert_eq!(a.panel, Some(ep_core::Panel::Main));
    let z = sim.actuator(ids::ZOOM).unwrap();
    assert_eq!((z.min, z.max, z.initval), (-0.5, 0.5, 0.0));
    let s = sim.controls().slider_of(ids::ZOOM + 4).unwrap();
    assert_eq!(s.id, ids::ZOOM);
    assert_eq!(sim.actuator(ids::ZOOM + 4).unwrap().max, 0.5);
    // Main panel 13..=114, colours 116..=214, texture 316..=345.
    assert_eq!(sim.actuators().count(), 102 + 99 + 30);
}
