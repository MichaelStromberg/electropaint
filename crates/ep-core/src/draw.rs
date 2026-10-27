//! `drawit` and `drawribbons` (rendering.md §4–§6): the strand, drawn from
//! the newest history slot to the oldest, each slot one transform step
//! further along.
//!
//! Every angle goes to IrisGL `rotate` as a short in tenths of a degree,
//! truncated toward zero from a double-precision `angle * 10.0`.

use crate::colour::hls_to_rgb;
use crate::panel::ids;
use crate::scene::{Ribbon, Shape, Tile};
use crate::sim::{RING, Sim, slot};

/// The square drawn per slot (`square[0..3]`).
const SQUARE: [[f32; 2]; 4] = [[-0.1, -0.1], [0.1, -0.1], [0.1, 0.1], [-0.1, 0.1]];
/// The triangle (`triangle[0..2]`); 0.173205078 is stored as `0x3e315cac`.
fn triangle() -> [[f32; 2]; 3] {
    [[0.0, 0.0], [0.2, 0.0], [0.1, f32::from_bits(0x3e31_5cac)]]
}
/// The square's normal (`ep_trinormal`).
const NORMAL: [f32; 3] = [0.0, 0.0, 1.0];

/// `twixt(k, a, t)`: interpolate between slot k−1 (weight 1−t) and slot k.
/// The first product is double precision, the second single.
fn twixt(a: &[f32; RING], k: usize, t: f32) -> f32 {
    let p = (k + RING - 1) % RING;
    (f64::from(a[p]) * (1.0 - f64::from(t)) + f64::from(a[k] * t)) as f32
}

/// `foldtwixt(k, a, t, per)`: like [`twixt`] for a periodic quantity, taking
/// the short way round when the two slots are more than half a period
/// apart. The result is not re-wrapped.
fn foldtwixt(a: &[f32; RING], k: usize, t: f32, per: f64) -> f32 {
    let per = per as f32;
    let p = (k + RING - 1) % RING;
    let (c, q) = (a[k], a[p]);
    let d = c - q;
    let w = 1.0 - f64::from(t);
    if f64::from(per) * 0.5 < f64::from(d) {
        (f64::from(q + per) * w + f64::from(c * t)) as f32
    } else if f64::from(d) < f64::from(-per) * 0.5 {
        (f64::from(q) * w + f64::from((c + per) * t)) as f32
    } else {
        twixt(a, k, t)
    }
}

/// A float angle in degrees to `rotate`'s tenths (truncating).
fn tenths(deg: f32, scale: f64) -> i16 {
    (f64::from(deg) * scale) as i32 as i16
}

/// The interpolated values of one slot.
struct SlotValues {
    x: f32,
    y: f32,
    z: f32,
    arm: f32,
    wrist: f32,
    size: f32,
    spin: f32,
    flip: f32,
    twist: f32,
    hue: f32,
    alpha: f32,
    light: f32,
    alpha_out: f32,
}

impl Sim {
    fn slot_values(&self, p: usize) -> SlotValues {
        let (h, t) = (&self.hist, self.t);
        SlotValues {
            x: twixt(&h.x, p, t),
            y: twixt(&h.y, p, t),
            z: twixt(&h.dzoom, p, t),
            arm: twixt(&h.arm, p, t),
            wrist: twixt(&h.wrist, p, t),
            size: twixt(&h.size, p, t),
            spin: foldtwixt(&h.spin, p, t, 360.0),
            flip: foldtwixt(&h.flip, p, t, 360.0),
            twist: twixt(&h.dtwist, p, t),
            hue: foldtwixt(&h.hue, p, t, 1.0),
            alpha: twixt(&h.alpha, p, t),
            light: foldtwixt(&h.light, p, t, 1.0),
            alpha_out: twixt(&h.alphaout, p, t),
        }
    }

    /// Fill and outline colours: the hue at full saturation, and the
    /// complementary hue at inverted lightness (computed with a second
    /// conversion, not as `1 − rgb`, so the rounding matches).
    fn slot_colours(&mut self, v: &SlotValues, single_wrap: bool) {
        let c = hls_to_rgb(f64::from(v.hue), f64::from(v.light), 1.0);
        self.col_rgba[..3].copy_from_slice(&c);
        let mut h2 = (f64::from(v.hue) + 0.5) as f32;
        // drawit compares and subtracts in double, drawribbons in single;
        // the results are identical (the subtraction is exact).
        if single_wrap {
            if 1.0 < h2 {
                h2 -= 1.0;
            }
        } else if 1.0 < f64::from(h2) {
            h2 = (f64::from(h2) - 1.0) as f32;
        }
        self.outline_rgba[3] = v.alpha_out;
        self.col_rgba[3] = v.alpha;
        let o = hls_to_rgb(f64::from(h2), 1.0 - f64::from(v.light), 1.0);
        self.outline_rgba[..3].copy_from_slice(&o);
    }

    /// The strand's slots, newest first.
    fn strand_slots(&self) -> impl Iterator<Item = usize> + use<> {
        let (n, lim) = (self.n, self.nlimit);
        ((n - lim + 1)..=n).rev().map(slot)
    }

    /// Wheel rotation and the centring pre-pass shared by both styles.
    fn begin_strand(&mut self) {
        self.gl.pushmatrix();
        self.gl.rotate(tenths(self.wheel, 10.0), 'x');
        for p in self.strand_slots() {
            let z = twixt(&self.hist.dzoom, p, self.t);
            self.gl.translate(0.0, 0.0, (f64::from(-z) * 0.5) as f32);
        }
    }

    /// The slot's local transform.
    fn slot_transform(&mut self, v: &SlotValues, rs: i16, rf: i16) {
        self.gl.translate(v.x, v.y, 0.0);
        self.gl.rotate(rs, 'z');
        self.gl.translate(0.0, v.arm, 0.0);
        self.gl.rotate(rf, 'y');
        self.gl.translate(v.wrist, 0.0, 0.0);
        self.gl.scale(v.size, v.size, 1.0);
    }

    /// Starts recording a new scene, if scenes are on: `drawit` may run
    /// twice in a step (smooth mode), and the picture left is the last.
    fn begin_scene(&mut self) {
        let effects = [ids::SMEAR, ids::SMOOTH, ids::FADE].map(|id| self.controls.act(id).on());
        if let Some(scene) = &mut self.scene {
            scene.tiles.clear();
            scene.ribbons.clear();
            scene.background = self.bkg_rgba;
            [scene.smear, scene.smooth, scene.fade] = effects;
            self.scene_drawn = true;
        }
    }

    /// Records the copy of slot `p` that the modelview now places, the
    /// `age`th slot along the strand.
    fn record_tile(&mut self, p: usize, v: &SlotValues, age: usize, copy: u8) {
        let Some(scene) = &mut self.scene else {
            return;
        };
        let shape = if self.controls.act(ids::TRIANGLES).on() {
            Shape::Triangle
        } else {
            Shape::Square
        };
        scene.tiles.push(Tile {
            model_view: *self.gl.modelview(),
            shape,
            fill: self.hist.fill[p].then_some(self.col_rgba),
            outline: self.hist.outline[p].then_some(self.outline_rgba),
            fat: self.hist.fatline[p],
            hue: v.hue,
            light: v.light,
            age: age as u16,
            copy,
        });
    }

    /// `drawit`: the default square (or triangle) strand.
    pub(crate) fn drawit(&mut self) {
        self.begin_scene();
        if self.controls.act(ids::RIBBON).on() {
            self.drawribbons();
            return;
        }
        if self.controls.act(ids::ZBUF).on() {
            self.gl.zclear();
        }
        let light_on = self.controls.act(ids::LIGHT).on();
        if light_on {
            self.uselight();
        }
        self.begin_strand();
        for (age, p) in self.strand_slots().enumerate() {
            let v = self.slot_values(p);
            let amb = twixt(&self.hist.ambient, p, self.t);
            let shiny = twixt(&self.hist.shiny, p, self.t);
            self.slot_colours(&v, false);
            // These call sites test "off" as `val < mid`.
            let lit = self.controls.act(ids::LIGHT).not_off();
            self.set_colour(self.col_rgba, lit, amb, shiny);
            self.gl.linewidth(if self.hist.fatline[p] { 3 } else { 1 });
            let rs = tenths(v.spin, 10.0);
            let rf = tenths(v.flip, 10.0);
            let copy = |c| (p, &v, (age, c));
            self.slot_copy(copy(0), rs, rf, lit, amb, shiny);
            if self.quad {
                // Copies 2-4 mirror in y, rotate by 180 degrees, mirror
                // again. The base matrix is left rotated by 180 degrees, so
                // the copy order alternates from slot to slot.
                self.gl.scale(1.0, -1.0, 1.0);
                self.slot_copy(copy(1), rs, rf, lit, amb, shiny);
                self.gl.rotate(1800, 'z');
                self.slot_copy(copy(2), rs, rf, lit, amb, shiny);
                self.gl.scale(1.0, -1.0, 1.0);
                self.slot_copy(copy(3), rs, rf, lit, amb, shiny);
            }
            self.gl.rotate(tenths(v.twist, 10.0), 'z');
            self.gl.translate(0.0, 0.0, v.z);
        }
        self.gl.popmatrix();
    }

    fn set_colour(&mut self, rgba: [f32; 4], lit: bool, amb: f32, shiny: f32) {
        if lit {
            self.lmcolor(rgba, Some(amb), shiny);
        } else {
            self.gl.c4f(rgba);
        }
    }

    /// One copy of a slot's shape: fill, then outline in the complementary
    /// colour, then the fill colour again. `slot` is the slot, its values,
    /// and its age and copy number for the scene.
    fn slot_copy(
        &mut self,
        slot: (usize, &SlotValues, (usize, u8)),
        rs: i16,
        rf: i16,
        lit: bool,
        amb: f32,
        shiny: f32,
    ) {
        let (p, v, (age, copy)) = slot;
        self.gl.pushmatrix();
        self.slot_transform(v, rs, rf);
        self.record_tile(p, v, age, copy);
        if self.controls.act(ids::LIGHT).on() {
            self.gl.n3f(NORMAL);
        }
        if self.hist.fill[p] {
            self.gl.bgnpolygon();
            self.shape_vertices();
            self.gl.endpolygon();
        }
        if self.hist.outline[p] {
            self.set_colour(self.outline_rgba, lit, amb, shiny);
            self.gl.bgnclosedline();
            self.shape_vertices();
            self.gl.endclosedline();
            self.set_colour(self.col_rgba, lit, amb, shiny);
        }
        self.gl.popmatrix();
    }

    /// `drawshape`'s vertices (untextured): the triangle when its toggle is
    /// on, else the square.
    fn shape_vertices(&mut self) {
        if self.controls.act(ids::TRIANGLES).on() {
            for v in triangle() {
                self.gl.v2f(v);
            }
        } else {
            for v in SQUARE {
                self.gl.v2f(v);
            }
        }
    }

    /// `drawribbons`: the strand as a triangle mesh, two vertices per slot.
    ///
    /// Quirks kept: the second vertex is `v3f` pointed into the middle of
    /// the triangle table, giving (0.2, 0, 0.1); with lighting on the
    /// material's ambient factor is uninitialised (logged as NaN) and no
    /// normal is ever sent; in quad mode `bgntmesh` is called once per copy
    /// but `endtmesh` only once, and copies 3 and 4 twist the other way.
    fn drawribbons(&mut self) {
        if self.controls.act(ids::ZBUF).on() {
            self.gl.zclear();
        }
        self.begin_strand();
        let copies = if self.quad { 4 } else { 1 };
        let tri = triangle();
        for c in 0..copies {
            let mut ribbon = self.scene.as_ref().map(|_| Ribbon::default());
            self.gl.pushmatrix();
            self.gl.bgntmesh();
            for p in self.strand_slots() {
                let v = self.slot_values(p);
                self.slot_colours(&v, true);
                if self.controls.act(ids::LIGHT).not_off() {
                    // The stack slots for ambient and shininess are never
                    // written; traces show shininess 0.
                    self.lmcolor(self.col_rgba, None, 0.0);
                } else {
                    self.gl.c4f(self.col_rgba);
                }
                self.gl.linewidth(if self.hist.fatline[p] { 3 } else { 1 });
                self.gl.pushmatrix();
                self.slot_transform(&v, tenths(v.spin, 10.0), tenths(v.flip, 10.0));
                let edge = [
                    [tri[0][0], tri[0][1], 0.0],
                    [tri[1][0], tri[1][1], tri[2][0]],
                ];
                if let Some(r) = &mut ribbon {
                    r.edges.push(edge.map(|e| self.gl.eye_point(e)));
                    r.colours.push(self.col_rgba);
                    r.hues.push(v.hue);
                }
                self.gl.v2f(tri[0]);
                self.gl.v3f(edge[1]);
                self.gl.popmatrix();
                let scale = if c < 2 { 10.0 } else { -10.0 };
                self.gl.rotate(tenths(v.twist, scale), 'z');
                self.gl.translate(0.0, 0.0, v.z);
            }
            self.gl.popmatrix();
            if let (Some(scene), Some(r)) = (&mut self.scene, ribbon) {
                scene.ribbons.push(r);
            }
            if c % 2 == 0 {
                self.gl.rotate(1800, 'z');
            } else {
                self.gl.scale(1.0, -1.0, 1.0);
            }
        }
        self.gl.endtmesh();
        self.gl.popmatrix();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twixt_mixes_neighbouring_slots() {
        let mut a = [0.0f32; RING];
        a[4] = 1.0;
        a[5] = 3.0;
        assert_eq!(twixt(&a, 5, 0.0), 1.0);
        assert_eq!(twixt(&a, 5, 0.5), 2.0);
        a[127] = 7.0;
        a[0] = 9.0;
        assert_eq!(twixt(&a, 0, 1.0), 9.0);
    }

    #[test]
    fn foldtwixt_takes_the_short_way() {
        let mut a = [0.0f32; RING];
        a[1] = -358.0;
        a[2] = 2.0;
        // From -358 to 2 the short way: the older slot is folded up by 360.
        assert_eq!(foldtwixt(&a, 2, 0.0, 360.0), 2.0);
        a[1] = 350.0;
        a[2] = -3.0;
        // And the newer one up when it is far below.
        assert_eq!(foldtwixt(&a, 2, 1.0, 360.0), 357.0);
        a[1] = 0.9;
        a[2] = 0.1;
        assert_eq!(
            foldtwixt(&a, 2, 0.5, 1.0),
            (f64::from(0.9f32) * 0.5 + f64::from(1.1f32 * 0.5)) as f32
        );
    }

    #[test]
    fn tenths_truncate() {
        assert_eq!(tenths(1.99, 10.0), 19);
        assert_eq!(tenths(-1.99, 10.0), -19);
        assert_eq!(tenths(60.0, -10.0), -600);
    }
}
