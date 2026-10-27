//! The scene recorded beside the drawing (`Options::scene`).

use electropaint_core::{Options, Sim, ids};

fn sim(scene: bool, quad: bool) -> Sim {
    let mut opts = Options::new(7);
    opts.call_log = true;
    opts.scene = scene;
    opts.quad = quad;
    Sim::new(opts)
}

/// Toggles pressed on the way through a run, to reach every drawing path.
const PRESSES: [(u32, u16); 10] = [
    (60, ids::TRIANGLES),
    (90, ids::FATLINE),
    (120, ids::RIBBON),
    (160, ids::LIGHT),
    (200, ids::RIBBON),
    (230, ids::SMEAR),
    (260, ids::SMOOTH),
    (300, ids::FADE),
    (330, ids::STOP),
    (340, ids::STOP),
];

#[test]
fn recording_the_scene_leaves_the_calls_and_frames_alone() {
    for quad in [false, true] {
        let (mut plain, mut with_scene) = (sim(false, quad), sim(true, quad));
        for tick in 0..400 {
            for &(at, id) in &PRESSES {
                if tick == at {
                    plain.press(id);
                    with_scene.press(id);
                }
            }
            let (a, b) = (plain.tick(), with_scene.tick());
            assert_eq!(a.ops, b.ops, "tick {tick}");
            assert!(a.scene.is_none());
            // Compared as text: lit ribbons log a NaN ambient on purpose,
            // and NaN is never equal to itself.
            let calls = |s: &mut Sim| format!("{:?}", s.take_calls());
            assert_eq!(calls(&mut plain), calls(&mut with_scene), "tick {tick}");
        }
    }
}

#[test]
fn every_slot_and_copy_is_a_tile() {
    for (quad, copies) in [(false, 1), (true, 4)] {
        let mut s = sim(true, quad);
        for _ in 0..100 {
            s.tick();
        }
        let scene = s.tick().scene.expect("a scene");
        let slots = s.strand().1 as usize;
        assert!(slots > 1);
        assert_eq!(scene.tiles.len(), slots * copies);
        assert!(scene.ribbons.is_empty());
        let ages: Vec<_> = scene.tiles.iter().map(|t| t.age).collect();
        assert!(ages.is_sorted() && ages[0] == 0, "newest first");
        assert!(scene.tiles.iter().all(|t| usize::from(t.copy) < copies));

        s.press(ids::RIBBON);
        s.tick();
        let scene = s.tick().scene.expect("a scene");
        assert!(scene.tiles.is_empty());
        assert_eq!(scene.ribbons.len(), copies);
        let slots = s.strand().1 as usize;
        assert!(scene.ribbons.iter().all(|r| r.edges.len() == slots));
    }
}

#[test]
fn in_betweens_have_scenes_and_effects_are_reported() {
    let mut s = sim(true, false);
    for _ in 0..50 {
        s.tick();
    }
    let between = s.frame_between(0.5).expect("an in-between");
    let scene = between.scene.expect("its scene");
    assert!(!scene.tiles.is_empty() && !scene.smear);

    s.press(ids::SMEAR);
    s.tick();
    let scene = s.tick().scene.expect("a scene");
    assert!(scene.smear && !scene.fade);
}

#[test]
fn a_stopped_step_draws_no_new_scene() {
    let mut s = sim(true, false);
    for _ in 0..50 {
        s.tick();
    }
    s.press(ids::STOP);
    // The press takes effect on the next step, which draws once more.
    let drawn = (0..4).filter(|_| s.tick().scene.is_some()).count();
    assert!(drawn <= 2, "{drawn}");
    assert!(s.tick().scene.is_none());
}

#[test]
fn switching_smear_on_still_records_the_step() {
    let mut s = sim(true, false);
    for _ in 0..50 {
        s.tick();
    }
    s.press(ids::SMEAR);
    // The press is handled in the next step, which then draws as smear.
    let scenes: Vec<_> = (0..3).map(|_| s.tick().scene.expect("a scene")).collect();
    assert!(scenes.iter().all(|sc| !sc.tiles.is_empty()));
    assert!(scenes.last().is_some_and(|sc| sc.smear));
}

#[test]
fn in_betweens_leave_the_next_scene_alone() {
    let (mut a, mut b) = (sim(true, false), sim(true, false));
    for _ in 0..50 {
        a.tick();
        b.tick();
    }
    assert!(a.frame_between(0.3).is_some());
    assert_eq!(a.tick().scene, b.tick().scene);
}
