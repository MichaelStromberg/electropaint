//! The geometry Remastered draws: a tile's bevelled slab, built once, and a
//! ribbon's band, built each picture from the scene. All on the CPU.

use ep_core::scene::{Ribbon, Shape};

use super::linear;

/// The slab: its edge radius (half its thickness), the radius of its
/// corners seen from the front, and how many segments make each curve.
const EDGE_RADIUS: f32 = 0.007;
const CORNER_RADIUS: f32 = 0.018;
const CORNER_SEGMENTS: usize = 4;
const EDGE_SEGMENTS: usize = 4;
/// How far a face's normals lean out at its edge (the tangent of the angle).
const DOME: f32 = 0.35;

/// Layout matches `MeshVertex` in remastered.metal.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct MeshVertex {
    /// Model space; w is how near the rim (0 at the centre, 1 at the edge).
    pub(super) pos: [f32; 4],
    pub(super) normal: [f32; 4],
}

// The shaders' layouts: a mismatch would draw garbage, not fail.
const _: () = assert!(size_of::<MeshVertex>() == 32 && size_of::<RibbonVertex>() == 48);

/// Layout matches `RibbonVertex` in remastered.metal.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct RibbonVertex {
    pub(super) pos: [f32; 4],
    /// w is the hue the film thickness follows.
    pub(super) normal: [f32; 4],
    pub(super) colour: [f32; 4],
}

/// Appends a ribbon's triangles: a quad between each pair of neighbouring
/// slots, flat shaded in the newer slot's colour.
pub(super) fn ribbon_strip(out: &mut Vec<RibbonVertex>, ribbon: &Ribbon, dx: f32) {
    let moved = |p: [f32; 3]| [p[0] + dx, p[1], p[2], 1.0];
    for (k, pair) in ribbon.edges.windows(2).enumerate() {
        let [newer, newer_far] = pair[0];
        let [older, older_far] = pair[1];
        let across = cross(sub(newer_far, newer), sub(older, newer));
        let norm = length(across);
        if norm < 1e-9 {
            continue;
        }
        let hue = ribbon.hues.get(k).copied().unwrap_or(0.0);
        let normal = [across[0] / norm, across[1] / norm, across[2] / norm, hue];
        let colour = linear(ribbon.colours[k]);
        for p in [newer, newer_far, older_far, newer, older_far, older] {
            out.push(RibbonVertex {
                pos: moved(p),
                normal,
                colour,
            });
        }
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1].mul_add(b[2], -a[2] * b[1]),
        a[2].mul_add(b[0], -a[0] * b[2]),
        a[0].mul_add(b[1], -a[1] * b[0]),
    ]
}

fn length(v: [f32; 3]) -> f32 {
    v[0].mul_add(v[0], v[1].mul_add(v[1], v[2] * v[2])).sqrt()
}

/// The tile `shape` as a closed slab: its outline with rounded corners,
/// faces front and back, and a rounded edge between them, as a triangle
/// list. The outline keeps ep's size, so the slab covers what ep's tile did.
pub(super) fn slab(shape: Shape) -> Vec<MeshVertex> {
    // Both shapes have an incircle: shrinking towards its centre offsets
    // every edge inwards by the same distance.
    let (corners, centre, inradius): (&[[f32; 2]], [f32; 2], f32) = match shape {
        Shape::Square => (
            &[[-0.1, -0.1], [0.1, -0.1], [0.1, 0.1], [-0.1, 0.1]],
            [0.0, 0.0],
            0.1,
        ),
        Shape::Triangle => {
            let h = 0.173_205_08;
            (&[[0.0, 0.0], [0.2, 0.0], [0.1, h]], [0.1, h / 3.0], h / 3.0)
        }
    };
    let shrink = (inradius - CORNER_RADIUS) / inradius;
    let inner: Vec<[f32; 2]> = corners
        .iter()
        .map(|c| {
            [
                centre[0] + (c[0] - centre[0]) * shrink,
                centre[1] + (c[1] - centre[1]) * shrink,
            ]
        })
        .collect();
    // The outward normal's angle of an edge, the corners going anticlockwise.
    let edge_normal = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).atan2(b[1] - a[1]);
    // The outline: around each corner, an arc between its edges' normals.
    let mut outline: Vec<([f32; 2], [f32; 2])> = Vec::new();
    let sides = inner.len();
    for i in 0..sides {
        let prev = inner[(i + sides - 1) % sides];
        let next = inner[(i + 1) % sides];
        let a0 = edge_normal(prev, inner[i]);
        let mut a1 = edge_normal(inner[i], next);
        if a1 < a0 {
            a1 += std::f32::consts::TAU;
        }
        for k in 0..=CORNER_SEGMENTS {
            let a = a0 + (a1 - a0) * k as f32 / CORNER_SEGMENTS as f32;
            outline.push((inner[i], [a.cos(), a.sin()]));
        }
    }
    // Rings from the front face's edge round to the back face's.
    let core = CORNER_RADIUS - EDGE_RADIUS;
    let ring = |j: usize| -> Vec<MeshVertex> {
        let phi =
            std::f32::consts::FRAC_PI_2 - std::f32::consts::PI * j as f32 / EDGE_SEGMENTS as f32;
        let (s, c) = phi.sin_cos();
        outline
            .iter()
            .map(|&(p, d)| MeshVertex {
                pos: [
                    p[0] + d[0] * (core + EDGE_RADIUS * c),
                    p[1] + d[1] * (core + EDGE_RADIUS * c),
                    EDGE_RADIUS * s,
                    1.0,
                ],
                normal: [d[0] * c, d[1] * c, s, 0.0],
            })
            .collect()
    };
    let rings: Vec<Vec<MeshVertex>> = (0..=EDGE_SEGMENTS).map(ring).collect();
    let mut out = Vec::new();
    let m = outline.len();
    for pair in rings.windows(2) {
        let [this, next] = [&pair[0], &pair[1]];
        for k in 0..m {
            let k1 = (k + 1) % m;
            out.extend([this[k], this[k1], next[k1], this[k], next[k1], next[k]]);
        }
    }
    // The faces are flat, but shaded as if gently domed (their normals lean
    // outwards towards the edge), so a reflection sweeps across each tile.
    for (ring, z) in [(&rings[0], 1.0f32), (&rings[EDGE_SEGMENTS], -1.0)] {
        let middle = MeshVertex {
            pos: [centre[0], centre[1], EDGE_RADIUS * z, 0.0],
            normal: [0.0, 0.0, z, 0.0],
        };
        let domed = |v: MeshVertex| {
            let lean = [
                (v.pos[0] - centre[0]) / inradius * DOME,
                (v.pos[1] - centre[1]) / inradius * DOME,
            ];
            let scale = length([lean[0], lean[1], 1.0]);
            MeshVertex {
                normal: [lean[0] / scale, lean[1] / scale, z / scale, 0.0],
                ..v
            }
        };
        for k in 0..m {
            out.extend([middle, domed(ring[k]), domed(ring[(k + 1) % m])]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slabs_keep_eps_outline() {
        // The edges lie on ep's (the corners are rounded inside them): the
        // square's sides at x = ±0.1, the triangle's base at y = 0.
        for (shape, axis, sign, edge) in [
            (Shape::Square, 0, 1.0, 0.1),
            (Shape::Triangle, 1, -1.0, 0.0),
        ] {
            let verts = slab(shape);
            assert_eq!(verts.len() % 3, 0);
            let extreme = verts
                .iter()
                .map(|v| sign * v.pos[axis])
                .fold(f32::MIN, f32::max);
            assert!((extreme - sign * edge).abs() < 1e-4, "{shape:?}: {extreme}");
            let depth = verts.iter().map(|v| v.pos[2].abs()).fold(0.0, f32::max);
            assert!((depth - EDGE_RADIUS).abs() < 1e-6);
            for v in &verts {
                let n = v.normal;
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-4, "{n:?}");
            }
        }
    }

    #[test]
    fn a_ribbon_is_a_quad_between_neighbouring_slots() {
        let edge = |z: f32| [[0.0, 0.0, z], [0.2, 0.0, z]];
        let ribbon = Ribbon {
            // The third slot repeats the second: a flat quad, left out.
            edges: vec![edge(0.0), edge(-0.1), edge(-0.1), edge(-0.2)],
            colours: vec![[1.0, 0.0, 0.0, 1.0]; 4],
            hues: vec![0.1, 0.2, 0.3, 0.4],
        };
        let mut out = Vec::new();
        ribbon_strip(&mut out, &ribbon, 1.0);
        assert_eq!(out.len(), 6 * 2);
        // Moved sideways, facing up or down, with the newer slot's hue.
        let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(
            out.iter()
                .all(|v| near(v.pos[0], 1.0) || near(v.pos[0], 1.2))
        );
        assert!(out.iter().all(|v| near(v.normal[1].abs(), 1.0)));
        assert!(near(out[0].normal[3], 0.1) && near(out[6].normal[3], 0.3));
    }
}
